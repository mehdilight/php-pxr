/*
 * PXR runtime library — milestone 0.
 *
 * This is the PXR "standalone runtime": every executable produced by `pxr build`
 * links against it, so PXR programs do not need the traditional PHP
 * interpreter to run.
 *
 * Design notes (see docs/RUNTIME.md):
 *   - Values are unboxed: int -> int64_t, float -> double, bool -> i1/_Bool,
 *     string -> `const char *` (NUL-terminated UTF-8/byte string).
 *   - String-producing operations allocate from a process-wide bump arena.
 *     There is no per-value free yet; owned strings / reference counting are a
 *     roadmap item. The arena is released by the operating system at exit.
 *   - This file is embedded into the `pxr` binary at build time and compiled
 *     with the system C compiler on demand, then cached.
 */

#include <errno.h>
#include <math.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* ------------------------------------------------------------------ arena */

/*
 * A page-list "bump" arena. Unlike a single realloc'd block, growing the
 * arena never invalidates previously handed-out pointers, which matters once
 * arrays (whose values may reference earlier strings) are in play. Memory is
 * handed to the operating system at exit; there is no per-value free yet.
 */
typedef struct pxr_page {
    struct pxr_page *next;
    size_t cap;
    size_t used;
    char data[]; /* 16-byte aligned on 64-bit targets */
} pxr_page;

static pxr_page *g_page = NULL;

static void pxr_oom(void) {
    fputs("pxr: out of memory\n", stderr);
    abort();
}

void *pxr_arena_alloc(size_t n) {
    n = (n + 7u) & ~(size_t)7u; /* 8-byte align */
    if (!g_page || g_page->used + n > g_page->cap) {
        size_t cap = n > ((size_t)1 << 16) ? n : ((size_t)1 << 16);
        pxr_page *p = (pxr_page *)malloc(sizeof(pxr_page) + cap);
        if (!p) {
            pxr_oom();
        }
        p->next = g_page;
        p->cap = cap;
        p->used = 0;
        g_page = p;
    }
    char *p = g_page->data + g_page->used;
    g_page->used += n;
    return p;
}

char *pxr_str_alloc(size_t len) {
    char *p = (char *)pxr_arena_alloc(len + 1);
    p[len] = '\0';
    return p;
}

/* -------------------------------------------------------------- printing */

void pxr_print_i64(long long v) {
    printf("%lld", v);
}

/*
 * PHP's default float stringification (zend_gcvt with precision=14):
 *   - 14 significant digits, correctly rounded;
 *   - scientific notation only when the decimal point lies outside [-3, 14];
 *   - an exponential mantissa always carries a decimal point ("1.0E+15");
 *   - the exponent is signed and never zero-padded ("1.0E-5");
 *   - integral values print without a fraction ("100", "2");
 *   - NAN/INF/-INF and "-0" match PHP.
 * `buf` must hold at least 48 bytes.
 */
static void pxr_gcvt(double v, char *buf) {
    if (v != v) {
        strcpy(buf, "NAN");
        return;
    }
    if (v == 1.0 / 0.0) {
        strcpy(buf, "INF");
        return;
    }
    if (v == -1.0 / 0.0) {
        strcpy(buf, "-INF");
        return;
    }
    if (v == 0.0) {
        strcpy(buf, signbit(v) ? "-0" : "0");
        return;
    }
    char tmp[48];
    snprintf(tmp, sizeof tmp, "%.13e", v);
    const char *p = tmp;
    char *o = buf;
    if (*p == '-') {
        *o++ = '-';
        p++;
    }
    char digits[16];
    int nd = 0;
    digits[nd++] = *p++; /* one leading digit */
    if (*p == '.') {
        p++;
        while (*p >= '0' && *p <= '9') {
            digits[nd++] = *p++;
        }
    }
    int exp10 = atoi(p + 1); /* skips 'e' (or 'E') and the sign */
    int decpt = exp10 + 1;
    while (nd > 1 && digits[nd - 1] == '0') {
        nd--; /* drop trailing zeros, PHP trims them */
    }
    if (decpt < -3 || decpt > 14) {
        *o++ = digits[0];
        *o++ = '.';
        if (nd > 1) {
            memcpy(o, digits + 1, (size_t)(nd - 1));
            o += nd - 1;
        } else {
            *o++ = '0';
        }
        *o++ = 'E';
        int e = decpt - 1;
        if (e < 0) {
            *o++ = '-';
            e = -e;
        } else {
            *o++ = '+';
        }
        o += sprintf(o, "%d", e);
        *o = '\0';
        return;
    }
    if (decpt <= 0) {
        *o++ = '0';
        *o++ = '.';
        while (decpt < 0) {
            *o++ = '0';
            decpt++;
        }
        memcpy(o, digits, (size_t)nd);
        o += nd;
        *o = '\0';
        return;
    }
    /* decpt in (0, 14] */
    if (decpt >= nd) {
        memcpy(o, digits, (size_t)nd);
        o += nd;
        for (int i = 0; i < decpt - nd; i++) {
            *o++ = '0';
        }
        *o = '\0';
    } else {
        memcpy(o, digits, (size_t)decpt);
        o += decpt;
        *o++ = '.';
        memcpy(o, digits + decpt, (size_t)(nd - decpt));
        o += nd - decpt;
        *o = '\0';
    }
}

void pxr_print_f64(double v) {
    char buf[48];
    pxr_gcvt(v, buf);
    fputs(buf, stdout);
}

void pxr_print_str(const char *s) {
    fputs(s ? s : "", stdout);
}

/* bool prints as "1" for true and "" for false, matching PHP. */
void pxr_print_bool(int v) {
    if (v) {
        fputc('1', stdout);
    }
}

/* ---------------------------------------------------------------- strings */

char *pxr_str_concat(const char *a, const char *b) {
    if (!a) a = "";
    if (!b) b = "";
    size_t la = strlen(a);
    size_t lb = strlen(b);
    char *out = pxr_str_alloc(la + lb);
    memcpy(out, a, la);
    memcpy(out + la, b, lb);
    return out;
}

char *pxr_i64_to_str(long long v) {
    char buf[32];
    int n = snprintf(buf, sizeof buf, "%lld", v);
    char *out = pxr_str_alloc((size_t)n);
    memcpy(out, buf, (size_t)n);
    return out;
}

char *pxr_f64_to_str(double v) {
    char buf[48];
    pxr_gcvt(v, buf);
    size_t n = strlen(buf);
    char *out = pxr_str_alloc(n);
    memcpy(out, buf, n);
    return out;
}

char *pxr_bool_to_str(int v) {
    char *out = pxr_str_alloc(v ? 1u : 0u);
    if (v) {
        out[0] = '1';
    }
    return out;
}

long long pxr_strlen(const char *s) {
    return s ? (long long)strlen(s) : 0;
}

/* PHP-like numeric string coercion: read the leading numeric prefix. */
long long pxr_str_to_i64(const char *s) {
    if (!s) return 0;
    char *end = NULL;
    long long v = strtoll(s, &end, 10);
    if (end == s) {
        /* Fall back to a float prefix, e.g. "3.9abc" -> 3. */
        double d = strtod(s, &end);
        if (end == s) return 0;
        return (long long)d;
    }
    return v;
}

double pxr_str_to_f64(const char *s) {
    if (!s) return 0.0;
    char *end = NULL;
    double v = strtod(s, &end);
    if (end == s) return 0.0;
    return v;
}

/* -------------------------------------------------------- truthiness / cmp */

int pxr_truthy_i64(long long v) {
    return v != 0;
}

int pxr_truthy_f64(double v) {
    /* NaN is truthy in PHP. */
    return v != 0.0;
}

int pxr_truthy_str(const char *s) {
    return s != NULL && s[0] != '\0' && !(s[0] == '0' && s[1] == '\0');
}

int pxr_str_eq(const char *a, const char *b) {
    return strcmp(a ? a : "", b ? b : "") == 0;
}

int pxr_str_cmp(const char *a, const char *b) {
    return strcmp(a ? a : "", b ? b : "");
}

static int pxr_is_space(int c) {
    return c == ' ' || c == '\t' || c == '\n' || c == '\r' || c == '\v' ||
           c == '\f';
}

/*
 * PHP's `is_numeric_string`: the whole string (surrounding whitespace allowed)
 * must be a decimal integer or float. Hex, `inf`/`nan` and `_` separators are
 * excluded. `*is_int` distinguishes an in-range integer string from a float
 * (an out-of-range integer string is treated as a float, as in PHP).
 */
static int pxr_scan_numeric(const char *s, int *is_int, long long *iv,
                            double *dv) {
    if (!s) {
        return 0;
    }
    const char *p = s;
    while (pxr_is_space((unsigned char)*p)) {
        p++;
    }
    const char *start = p;
    if (*p == '+' || *p == '-') {
        p++;
    }
    int digits = 0, dot = 0, exp = 0;
    while (*p >= '0' && *p <= '9') {
        p++;
        digits++;
    }
    if (*p == '.') {
        dot = 1;
        p++;
        while (*p >= '0' && *p <= '9') {
            p++;
            digits++;
        }
    }
    if (digits == 0) {
        return 0;
    }
    if (*p == 'e' || *p == 'E') {
        const char *save = p;
        p++;
        if (*p == '+' || *p == '-') {
            p++;
        }
        int ed = 0;
        while (*p >= '0' && *p <= '9') {
            p++;
            ed++;
        }
        if (ed == 0) {
            p = save; /* not a real exponent: the trailing check will fail */
        } else {
            exp = 1;
        }
    }
    const char *end = p;
    while (pxr_is_space((unsigned char)*p)) {
        p++;
    }
    if (*p != '\0') {
        return 0;
    }
    if (!dot && !exp) {
        errno = 0;
        char *e2 = NULL;
        long long v = strtoll(start, &e2, 10);
        if (errno != ERANGE && e2 == end) {
            *is_int = 1;
            *iv = v;
            return 1;
        }
        /* out-of-range integer string -> compare as a float, like PHP */
    }
    *is_int = 0;
    *dv = strtod(start, NULL);
    return 1;
}

/*
 * Loose comparison of two strings, matching PHP: if both are numeric strings
 * they are compared as numbers, otherwise byte-wise. Returns -1, 0 or 1.
 */
long long pxr_str_cmp_loose(const char *a, const char *b) {
    const char *x = a ? a : "";
    const char *y = b ? b : "";
    int xi = 0, yi = 0;
    long long xil = 0, yil = 0;
    double xd = 0, yd = 0;
    if (pxr_scan_numeric(x, &xi, &xil, &xd) &&
        pxr_scan_numeric(y, &yi, &yil, &yd)) {
        if (xi && yi) {
            return xil < yil ? -1 : (xil > yil ? 1 : 0);
        }
        double lx = xi ? (double)xil : xd;
        double ly = yi ? (double)yil : yd;
        return lx < ly ? -1 : (lx > ly ? 1 : 0);
    }
    int c = strcmp(x, y);
    return c < 0 ? -1 : (c > 0 ? 1 : 0);
}

/* ------------------------------------------------------------------ math */

long long pxr_abs_i64(long long v) {
    /* Avoid UB for LLONG_MIN. */
    return v < 0 ? (long long)(0u - (unsigned long long)v) : v;
}

/*
 * PHP raises an uncaught `ArithmeticError` for a negative bit-shift count,
 * which is a fatal error (exit status 255). We reproduce the observable part
 * (message + exit status); the exact location/stack-trace text is Zend-specific
 * and is not reproduced.
 */
static void pxr_arith_error(const char *msg) {
    fprintf(stderr, "Fatal error: Uncaught ArithmeticError: %s\n", msg);
    exit(255);
}

/* PHP: shift counts >= 64 yield 0, and negative counts are a fatal error. */
long long pxr_shl(long long a, long long b) {
    if (b < 0) pxr_arith_error("Bit shift by negative number");
    if (b >= 64) return 0;
    return (long long)((unsigned long long)a << (unsigned)b);
}

long long pxr_shr(long long a, long long b) {
    if (b < 0) pxr_arith_error("Bit shift by negative number");
    if (b >= 64) return a < 0 ? -1 : 0;
    return a >> (unsigned)b;
}

/* PHP: division and remainder by zero raise an uncaught DivisionByZeroError. */
static void pxr_div_error(void) {
    fprintf(stderr, "Fatal error: Uncaught DivisionByZeroError: Division by zero\n");
    exit(255);
}

double pxr_div(double a, double b) {
    if (b == 0.0) {
        pxr_div_error();
    }
    return a / b;
}

long long pxr_mod(long long a, long long b) {
    if (b == 0) {
        pxr_div_error();
    }
    return a % b;
}

/* ----------------------------------------------------------------- arrays */

/*
 * PHP arrays are ordered maps: insertion order is preserved and keys are
 * either `int` or `string`. PXR statically types the *element* type, so a
 * given array stores a single element kind, but the key kind is dynamic at
 * run time (tag 1 = int, tag 4 = string), matching how the key helpers are
 * lowered.
 *
 * Values cross the ABI boundary as a type tag plus a 64-bit payload:
 *
 *   tag 0 = null, 1 = int, 2 = float, 3 = bool, 4 = string, 5 = array
 *
 * The compiler knows the static element type, so it is the one that decodes
 * the payload; the runtime just carries it.
 */

typedef enum {
    PXR_TAG_NULL = 0,
    PXR_TAG_INT = 1,
    PXR_TAG_FLOAT = 2,
    PXR_TAG_BOOL = 3,
    PXR_TAG_STR = 4,
    PXR_TAG_ARR = 5
} pxr_tag;

typedef struct {
    int64_t vt;   /* value tag */
    int64_t bits; /* value payload */
    char   *ks;   /* string key, or NULL for an integer key */
    int64_t ki;   /* integer key (valid when ks == NULL) */
} pxr_ent;

typedef struct {
    pxr_ent *ents;
    size_t   len;
    size_t   cap;
} pxr_array;

/*
 * PHP converts a *canonical* decimal-integer string key to an `int` key, so
 * `$a["1"]` and `$a[1]` address the same element. The canonical form is an
 * optional `-`, then either `0` or a non-zero digit followed by digits (no
 * leading zeros, no `+`), and the value must fit in a signed 64-bit integer.
 */
static int pxr_key_parse_int(const char *s, int64_t *out) {
    if (!s || !*s) {
        return 0;
    }
    const char *p = s;
    int neg = 0;
    if (*p == '-') {
        neg = 1;
        p++;
    }
    if (*p < '0' || *p > '9') {
        return 0;
    }
    if (*p == '0' && p[1] != '\0') {
        return 0; /* leading zero, e.g. "01" or "00" */
    }
    unsigned long long acc = 0;
    for (; *p; p++) {
        if (*p < '0' || *p > '9') {
            return 0;
        }
        unsigned d = (unsigned)(*p - '0');
        if (acc > (0xFFFFFFFFFFFFFFFFULL - d) / 10ULL) {
            return 0; /* would overflow 64 bits */
        }
        acc = acc * 10ULL + d;
    }
    if (neg) {
        if (acc > 0x8000000000000000ULL) {
            return 0; /* below INT64_MIN */
        }
        *out = (acc == 0x8000000000000000ULL) ? INT64_MIN : -(int64_t)acc;
    } else {
        if (acc > 0x7FFFFFFFFFFFFFFFULL) {
            return 0; /* above INT64_MAX */
        }
        *out = (int64_t)acc;
    }
    return 1;
}

/* Rewrite a canonical integer string key into an integer key in place. */
static void pxr_norm_key(long long *ktag, long long *ki, const char **ks) {
    if (*ktag == PXR_TAG_STR && *ks && *ks[0]) {
        int64_t v;
        if (pxr_key_parse_int(*ks, &v)) {
            *ktag = PXR_TAG_INT;
            *ki = v;
            *ks = NULL;
        }
    }
}

/* Linear scan; a hash index is a roadmap item. */
static long pxr_array_find(pxr_array *a, long long ktag, long long ki, const char *ks) {
    for (size_t i = 0; i < a->len; i++) {
        pxr_ent *e = &a->ents[i];
        if (e->ks == NULL) {
            if (ktag == PXR_TAG_INT && e->ki == (int64_t)ki) {
                return (long)i;
            }
        } else if (ktag == PXR_TAG_STR) {
            if (strcmp(e->ks, ks ? ks : "") == 0) {
                return (long)i;
            }
        }
    }
    return -1;
}

static void pxr_array_grow(pxr_array *a, size_t at_least) {
    size_t ncap = a->cap ? a->cap * 2 : 8;
    while (ncap < at_least) {
        ncap *= 2;
    }
    pxr_ent *ne = (pxr_ent *)realloc(a->ents, ncap * sizeof(pxr_ent));
    if (!ne) {
        pxr_oom();
    }
    a->ents = ne;
    a->cap = ncap;
}

void *pxr_array_new(void) {
    pxr_array *a = (pxr_array *)pxr_arena_alloc(sizeof(pxr_array));
    a->ents = NULL;
    a->len = 0;
    a->cap = 0;
    return a;
}

void pxr_array_set(void *p, long long ktag, long long ki, const char *ks,
                   long long vt, long long bits) {
    pxr_array *a = (pxr_array *)p;
    pxr_norm_key(&ktag, &ki, &ks);
    long idx = pxr_array_find(a, ktag, ki, ks);
    if (idx >= 0) {
        a->ents[idx].vt = vt;
        a->ents[idx].bits = bits;
        return;
    }
    if (a->len == a->cap) {
        pxr_array_grow(a, a->len + 1);
    }
    pxr_ent *e = &a->ents[a->len++];
    e->vt = vt;
    e->bits = bits;
    if (ktag == PXR_TAG_INT) {
        e->ks = NULL;
        e->ki = (int64_t)ki;
    } else {
        e->ks = (char *)(ks ? ks : "");
        e->ki = 0;
    }
}

void pxr_array_push(void *p, long long vt, long long bits) {
    pxr_array *a = (pxr_array *)p;
    /* PHP: the next key is the largest integer key plus one (0 if none). */
    int64_t nk = 0;
    for (size_t i = 0; i < a->len; i++) {
        if (a->ents[i].ks == NULL && a->ents[i].ki >= nk) {
            nk = a->ents[i].ki + 1;
        }
    }
    pxr_array_set(p, PXR_TAG_INT, nk, NULL, vt, bits);
}

long long pxr_array_get_bits(void *p, long long ktag, long long ki, const char *ks) {
    pxr_array *a = (pxr_array *)p;
    pxr_norm_key(&ktag, &ki, &ks);
    long idx = pxr_array_find(a, ktag, ki, ks);
    return idx < 0 ? 0 : a->ents[idx].bits;
}

/* Returns 1 when the key exists, 0 otherwise (used by `??` / `??=`). */
long long pxr_array_has(void *p, long long ktag, long long ki, const char *ks) {
    pxr_array *a = (pxr_array *)p;
    pxr_norm_key(&ktag, &ki, &ks);
    return pxr_array_find(a, ktag, ki, ks) >= 0 ? 1 : 0;
}

long long pxr_array_count(void *p) {
    pxr_array *a = (pxr_array *)p;
    return (long long)a->len;
}

long long pxr_array_val_bits_at(void *p, long long i) {
    pxr_array *a = (pxr_array *)p;
    if (i < 0 || (size_t)i >= a->len) {
        return 0;
    }
    return a->ents[(size_t)i].bits;
}

/*
 * PHP arrays are value types: assignment, passing and returning all copy, and
 * `foreach` iterates a snapshot. PXR realises this with an eager deep copy at
 * every place an array value is stored, passed, returned or iterated over.
 * String keys and string values are immutable, so they are shared; nested
 * arrays are copied recursively. Cycles cannot occur because PXR has no
 * references.
 */
void *pxr_array_copy(void *p) {
    pxr_array *s = (pxr_array *)p;
    pxr_array *d = (pxr_array *)pxr_array_new();
    if (s->len) {
        pxr_array_grow(d, s->len);
    }
    for (size_t i = 0; i < s->len; i++) {
        pxr_ent *src = &s->ents[i];
        int64_t bits = src->bits;
        if (src->vt == PXR_TAG_ARR && bits) {
            bits = (int64_t)(intptr_t)pxr_array_copy((void *)(intptr_t)bits);
        }
        d->ents[d->len].vt = src->vt;
        d->ents[d->len].bits = bits;
        d->ents[d->len].ks = src->ks;
        d->ents[d->len].ki = src->ki;
        d->len++;
    }
    return d;
}
