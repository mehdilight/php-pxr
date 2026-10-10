/*
 * PXR runtime library — milestone 0.
 *
 * This is the PXR "standalone runtime": every executable produced by `pxr build`
 * links against it, so PXR programs do not need the traditional PHP
 * interpreter to run.
 *
 * Design notes (see docs/RUNTIME.md):
 *   - Values are unboxed: int -> int64_t, float -> double, bool -> i1/_Bool,
 *     string -> `pxr_str *` (length-prefixed byte string; NUL- and binary-safe).
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

/*
 * A length-prefixed byte string, matching PHP's byte-string semantics:
 * `len` is the number of bytes, and `data` is exactly `len` bytes followed by a
 * NUL terminator that is *not* part of the string. Embedded NULs are therefore
 * representable (`strlen("a\0b") == 3`), unlike a bare `char *`.
 *
 * The ABI stays a single pointer (like the old `char *`), so the LLVM
 * declarations and codegen call sites are unchanged; only the allocation
 * layout differs. `data` sits at offset 8 (after `size_t len`).
 */
typedef struct {
    size_t len;
    char data[];
} pxr_str;

pxr_str *pxr_str_alloc(size_t len) {
    pxr_str *p = (pxr_str *)pxr_arena_alloc(sizeof(pxr_str) + len + 1);
    p->len = len;
    p->data[len] = '\0';
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

void pxr_print_str(const pxr_str *s) {
    if (s && s->len) {
        fwrite(s->data, 1, s->len, stdout);
    }
}

/* bool prints as "1" for true and "" for false, matching PHP. */
void pxr_print_bool(int v) {
    if (v) {
        fputc('1', stdout);
    }
}

/* ---------------------------------------------------------------- strings */

pxr_str *pxr_str_concat(const pxr_str *a, const pxr_str *b) {
    size_t la = a ? a->len : 0;
    size_t lb = b ? b->len : 0;
    pxr_str *out = pxr_str_alloc(la + lb);
    if (a) {
        memcpy(out->data, a->data, la);
    }
    if (b) {
        memcpy(out->data + la, b->data, lb);
    }
    return out;
}

pxr_str *pxr_i64_to_str(long long v) {
    char buf[32];
    int n = snprintf(buf, sizeof buf, "%lld", v);
    pxr_str *out = pxr_str_alloc((size_t)n);
    memcpy(out->data, buf, (size_t)n);
    return out;
}

pxr_str *pxr_f64_to_str(double v) {
    char buf[48];
    pxr_gcvt(v, buf);
    size_t n = strlen(buf);
    pxr_str *out = pxr_str_alloc(n);
    memcpy(out->data, buf, n);
    return out;
}

pxr_str *pxr_bool_to_str(int v) {
    pxr_str *out = pxr_str_alloc(v ? 1u : 0u);
    if (v) {
        out->data[0] = '1';
    }
    return out;
}

long long pxr_strlen(const pxr_str *s) {
    return s ? (long long)s->len : 0;
}

/* PHP-like numeric string coercion: read the leading numeric prefix. */
long long pxr_str_to_i64(const pxr_str *s) {
    if (!s) return 0;
    char *end = NULL;
    long long v = strtoll(s->data, &end, 10);
    if (end == s->data) {
        /* Fall back to a float prefix, e.g. "3.9abc" -> 3. */
        double d = strtod(s->data, &end);
        if (end == s->data) return 0;
        return (long long)d;
    }
    return v;
}

double pxr_str_to_f64(const pxr_str *s) {
    if (!s) return 0.0;
    char *end = NULL;
    double v = strtod(s->data, &end);
    if (end == s->data) return 0.0;
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

int pxr_truthy_str(const pxr_str *s) {
    if (!s || s->len == 0) {
        return 0;
    }
    /* PHP: only "" and "0" are falsy. */
    return !(s->len == 1 && s->data[0] == '0');
}

int pxr_str_eq(const pxr_str *a, const pxr_str *b) {
    size_t la = a ? a->len : 0;
    size_t lb = b ? b->len : 0;
    if (la != lb) {
        return 0;
    }
    return la == 0 || memcmp(a->data, b->data, la) == 0;
}

/* Byte-wise comparison (PHP's rule for non-numeric strings): compare the
 * common prefix, then the shorter string sorts first. */
int pxr_str_cmp(const pxr_str *a, const pxr_str *b) {
    size_t la = a ? a->len : 0;
    size_t lb = b ? b->len : 0;
    size_t n = la < lb ? la : lb;
    if (n) {
        int c = memcmp(a->data, b->data, n);
        if (c != 0) {
            return c < 0 ? -1 : 1;
        }
    }
    return la < lb ? -1 : (la > lb ? 1 : 0);
}

static int pxr_is_space(int c) {
    return c == ' ' || c == '\t' || c == '\n' || c == '\r' || c == '\v' ||
           c == '\f';
}

/*
 * PHP's `is_numeric_string`: the whole string (surrounding whitespace allowed)
 * must be a decimal integer or float. Hex, `inf`/`nan` and `_` separators are
 * excluded. `*is_int` distinguishes an in-range integer string from a float
 * (an out-of-range integer string is treated as a float, as in PHP). The scan
 * is bounded by the string's length, so embedded NULs terminate it exactly as
 * they do for PHP.
 */
static int pxr_scan_numeric(const pxr_str *s, int *is_int, long long *iv,
                            double *dv) {
    if (!s) {
        return 0;
    }
    const char *p = s->data;
    const char *endp = s->data + s->len;
    while (p < endp && pxr_is_space((unsigned char)*p)) {
        p++;
    }
    const char *start = p;
    if (p < endp && (*p == '+' || *p == '-')) {
        p++;
    }
    int digits = 0, dot = 0, exp = 0;
    while (p < endp && *p >= '0' && *p <= '9') {
        p++;
        digits++;
    }
    if (p < endp && *p == '.') {
        dot = 1;
        p++;
        while (p < endp && *p >= '0' && *p <= '9') {
            p++;
            digits++;
        }
    }
    if (digits == 0) {
        return 0;
    }
    if (p < endp && (*p == 'e' || *p == 'E')) {
        const char *save = p;
        p++;
        if (p < endp && (*p == '+' || *p == '-')) {
            p++;
        }
        int ed = 0;
        while (p < endp && *p >= '0' && *p <= '9') {
            p++;
            ed++;
        }
        if (ed == 0) {
            p = save; /* not a real exponent: the trailing check will fail */
        } else {
            exp = 1;
        }
    }
    const char *num_end = p;
    while (p < endp && pxr_is_space((unsigned char)*p)) {
        p++;
    }
    if (p != endp) {
        return 0;
    }
    if (!dot && !exp) {
        errno = 0;
        char *e2 = NULL;
        long long v = strtoll(start, &e2, 10);
        if (errno != ERANGE && e2 == num_end) {
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
long long pxr_str_cmp_loose(const pxr_str *a, const pxr_str *b) {
    int xi = 0, yi = 0;
    long long xil = 0, yil = 0;
    double xd = 0, yd = 0;
    if (pxr_scan_numeric(a, &xi, &xil, &xd) &&
        pxr_scan_numeric(b, &yi, &yil, &yd)) {
        if (xi && yi) {
            return xil < yil ? -1 : (xil > yil ? 1 : 0);
        }
        double lx = xi ? (double)xil : xd;
        double ly = yi ? (double)yil : yd;
        return lx < ly ? -1 : (lx > ly ? 1 : 0);
    }
    int c = pxr_str_cmp(a, b);
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
    int64_t vt;    /* value tag */
    int64_t bits;  /* value payload */
    pxr_str *ks;   /* string key, or NULL for an integer key */
    int64_t ki;    /* integer key (valid when ks == NULL) */
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
static int pxr_key_parse_int(const pxr_str *s, int64_t *out) {
    if (!s || s->len == 0) {
        return 0;
    }
    const char *p = s->data;
    const char *end = s->data + s->len;
    int neg = 0;
    if (*p == '-') {
        neg = 1;
        p++;
    }
    if (p >= end || *p < '0' || *p > '9') {
        return 0;
    }
    if (*p == '0' && p + 1 != end) {
        return 0; /* leading zero, e.g. "01" or "00" */
    }
    unsigned long long acc = 0;
    for (; p < end; p++) {
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
static void pxr_norm_key(long long *ktag, long long *ki, const pxr_str **ks) {
    if (*ktag == PXR_TAG_STR && *ks && (*ks)->len > 0) {
        int64_t v;
        if (pxr_key_parse_int(*ks, &v)) {
            *ktag = PXR_TAG_INT;
            *ki = v;
            *ks = NULL;
        }
    }
}

/* Linear scan; a hash index is a roadmap item. */
static long pxr_array_find(pxr_array *a, long long ktag, long long ki,
                           const pxr_str *ks) {
    for (size_t i = 0; i < a->len; i++) {
        pxr_ent *e = &a->ents[i];
        if (e->ks == NULL) {
            if (ktag == PXR_TAG_INT && e->ki == (int64_t)ki) {
                return (long)i;
            }
        } else if (ktag == PXR_TAG_STR && ks != NULL) {
            if (pxr_str_eq(e->ks, ks)) {
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

void pxr_array_set(void *p, long long ktag, long long ki, const pxr_str *ks,
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
        e->ks = (pxr_str *)ks;
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

long long pxr_array_get_bits(void *p, long long ktag, long long ki, const pxr_str *ks) {
    pxr_array *a = (pxr_array *)p;
    pxr_norm_key(&ktag, &ki, &ks);
    long idx = pxr_array_find(a, ktag, ki, ks);
    return idx < 0 ? 0 : a->ents[idx].bits;
}

/* Returns 1 when the key exists, 0 otherwise (used by `??` / `??=`). */
long long pxr_array_has(void *p, long long ktag, long long ki, const pxr_str *ks) {
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

/* ------------------------------------------------- M2 string builtins ----
 *
 * All of these reproduce PHP's exact byte-level behaviour, verified against
 * PHP 8.4. Byte strings are length-prefixed, so embedded NULs flow through
 * every operation below.
 */

pxr_str *pxr_str_toupper(const pxr_str *s) {
    pxr_str *out = pxr_str_alloc(s->len);
    for (size_t i = 0; i < s->len; i++) {
        unsigned char c = (unsigned char)s->data[i];
        out->data[i] = (char)((c >= 'a' && c <= 'z') ? (unsigned char)(c - 32) : c);
    }
    return out;
}

pxr_str *pxr_str_tolower(const pxr_str *s) {
    pxr_str *out = pxr_str_alloc(s->len);
    for (size_t i = 0; i < s->len; i++) {
        unsigned char c = (unsigned char)s->data[i];
        out->data[i] = (char)((c >= 'A' && c <= 'Z') ? (unsigned char)(c + 32) : c);
    }
    return out;
}

/* PHP's default trim mask: space, tab, newline, carriage return, NUL and
 * vertical tab. A non-NULL `mask` replaces it entirely (an empty mask strips
 * nothing, matching PHP). */
static int pxr_trim_char(unsigned char c, const pxr_str *mask) {
    if (mask) {
        for (size_t i = 0; i < mask->len; i++) {
            if ((unsigned char)mask->data[i] == c) {
                return 1;
            }
        }
        return 0;
    }
    switch (c) {
        case ' ':
        case '\t':
        case '\n':
        case '\r':
        case '\0':
        case '\v':
            return 1;
        default:
            return 0;
    }
}

/* mode: 0 = trim both ends, 1 = trim left only, 2 = trim right only. */
pxr_str *pxr_str_trim(const pxr_str *s, const pxr_str *mask, long long mode) {
    size_t begin = 0, end = s->len;
    if (mode != 2) {
        while (begin < end && pxr_trim_char((unsigned char)s->data[begin], mask)) {
            begin++;
        }
    }
    if (mode != 1) {
        while (end > begin && pxr_trim_char((unsigned char)s->data[end - 1], mask)) {
            end--;
        }
    }
    pxr_str *out = pxr_str_alloc(end - begin);
    memcpy(out->data, s->data + begin, end - begin);
    return out;
}

/* PHP 8 `substr`: offsets clamp, negative length cuts from the end, and
 * out-of-range results are the empty string (never `false`). */
pxr_str *pxr_substr(const pxr_str *s, long long f, long long l, long long has_len) {
    long long len = (long long)(s ? s->len : 0);
    if (f < 0) {
        f = len + f;
        if (f < 0) {
            f = 0;
        }
    }
    long long length;
    if (has_len) {
        length = l;
        if (length < 0) {
            length = len + length - f;
            if (length < 0) {
                length = 0;
            }
        } else if (length > len - f) {
            length = len - f;
        }
    } else {
        length = len - f;
    }
    if (f > len || f == len || length <= 0) {
        return pxr_str_alloc(0);
    }
    pxr_str *out = pxr_str_alloc((size_t)length);
    memcpy(out->data, s->data + f, (size_t)length);
    return out;
}

pxr_str *pxr_str_repeat(const pxr_str *s, long long n) {
    if (n < 0) {
        fprintf(stderr,
                "Fatal error: Uncaught ValueError: str_repeat(): Argument #2 "
                "($times) must be greater than or equal to 0\n");
        exit(255);
    }
    if (n == 0 || s->len == 0) {
        return pxr_str_alloc(0);
    }
    size_t sl = s->len;
    pxr_str *out = pxr_str_alloc(sl * (size_t)n);
    for (long long i = 0; i < n; i++) {
        memcpy(out->data + (size_t)i * sl, s->data, sl);
    }
    return out;
}

/* PHP `str_replace` with string arguments: replace every non-overlapping
 * occurrence scanning left to right, without rescanning replacement text. An
 * empty search returns the subject unchanged. */
pxr_str *pxr_str_replace(const pxr_str *hay, const pxr_str *rep, const pxr_str *subj) {
    size_t hl = hay ? hay->len : 0;
    size_t rl = rep ? rep->len : 0;
    size_t sl = subj ? subj->len : 0;
    if (hl == 0) {
        return (pxr_str *)subj;
    }
    size_t count = 0;
    const char *p = subj->data;
    const char *end = subj->data + sl;
    while ((size_t)(end - p) >= hl) {
        if (memcmp(p, hay->data, hl) == 0) {
            count++;
            p += hl;
        } else {
            p++;
        }
    }
    if (count == 0) {
        return (pxr_str *)subj;
    }
    pxr_str *out = pxr_str_alloc(sl + count * (rl - hl));
    char *o = out->data;
    p = subj->data;
    while ((size_t)(end - p) >= hl) {
        if (memcmp(p, hay->data, hl) == 0) {
            memcpy(o, rep->data, rl);
            o += rl;
            p += hl;
        } else {
            *o++ = *p++;
        }
    }
    memcpy(o, p, (size_t)(end - p));
    return out;
}

/* Unpack a float stored as its IEEE-754 bit pattern in an i64 payload. */
static double pxr_bits_to_double(long long bits) {
    double d;
    memcpy(&d, &bits, sizeof d);
    return d;
}

/* String form of an array value by its runtime tag and payload. */
static pxr_str *pxr_val_to_str(long long tag, long long bits) {
    switch (tag) {
        case PXR_TAG_INT:
            return pxr_i64_to_str((long long)bits);
        case PXR_TAG_FLOAT:
            return pxr_f64_to_str(pxr_bits_to_double(bits));
        case PXR_TAG_BOOL:
            return pxr_bool_to_str((int)bits);
        default: /* PXR_TAG_STR */
            return (pxr_str *)(intptr_t)bits;
    }
}

/* `implode`: join array values in insertion order with `glue`, coercing each
 * scalar element exactly as PHP does (`(string)` cast; floats via zend_gcvt). */
pxr_str *pxr_implode(const pxr_str *glue, void *p) {
    pxr_array *a = (pxr_array *)p;
    size_t gl = glue ? glue->len : 0;
    size_t total = a->len ? (a->len - 1) * gl : 0;
    for (size_t i = 0; i < a->len; i++) {
        pxr_str *v = pxr_val_to_str(a->ents[i].vt, a->ents[i].bits);
        total += v->len;
    }
    pxr_str *out = pxr_str_alloc(total);
    size_t o = 0;
    for (size_t i = 0; i < a->len; i++) {
        if (i > 0) {
            memcpy(out->data + o, glue->data, gl);
            o += gl;
        }
        pxr_str *v = pxr_val_to_str(a->ents[i].vt, a->ents[i].bits);
        memcpy(out->data + o, v->data, v->len);
        o += v->len;
    }
    return out;
}

/* Find the first occurrence of `needle` in `data[0..n)` (no built-in memmem). */
static const char *pxr_find(const char *data, size_t n, const char *needle, size_t nl) {
    if (nl == 0 || nl > n) {
        return NULL;
    }
    for (size_t i = 0; i + nl <= n; i++) {
        if (memcmp(data + i, needle, nl) == 0) {
            return data + i;
        }
    }
    return NULL;
}

static void pxr_explode_push(pxr_array *out, const char *data, size_t len) {
    pxr_str *piece = pxr_str_alloc(len);
    memcpy(piece->data, data, len);
    pxr_array_push(out, PXR_TAG_STR, (long long)(intptr_t)piece);
}

/* `explode`: split by a non-empty separator. Positive limits keep the rest of
 * the string as the final piece; `0` means `1`; negative limits drop the last
 * |limit| pieces. Trailing empty pieces are preserved exactly like PHP. */
void *pxr_explode(const pxr_str *sep, const pxr_str *s, long long limit) {
    pxr_str *es = (pxr_str *)sep;
    pxr_str *st = (pxr_str *)s;
    if (!es || es->len == 0) {
        fprintf(stderr,
                "Fatal error: Uncaught ValueError: explode(): Argument #1 "
                "($separator) must not be empty\n");
        exit(255);
    }
    pxr_array *out = (pxr_array *)pxr_array_new();
    size_t sl = st ? st->len : 0;
    size_t dl = es->len;
    const char *data = st ? st->data : "";
    const char *end = data + sl;
    if (limit == 0) {
        limit = 1;
    }
    if (limit > 0) {
        const char *p = data;
        long long n = 0;
        while (n < limit - 1) {
            const char *hit = pxr_find(p, (size_t)(end - p), es->data, dl);
            if (!hit) {
                break;
            }
            pxr_explode_push(out, p, (size_t)(hit - p));
            p = hit + dl;
            n++;
        }
        pxr_explode_push(out, p, (size_t)(end - p));
        return out;
    }
    /* Negative limit: split fully, then keep all but the last |limit| pieces. */
    long long total = 1;
    const char *p = data;
    while (p < end) {
        const char *hit = pxr_find(p, (size_t)(end - p), es->data, dl);
        if (!hit) {
            break;
        }
        total++;
        p = hit + dl;
    }
    long long keep = total + limit; /* limit < 0 */
    if (keep <= 0) {
        return out;
    }
    p = data;
    long long n = 0;
    while (p < end && n < keep) {
        const char *hit = pxr_find(p, (size_t)(end - p), es->data, dl);
        if (hit) {
            pxr_explode_push(out, p, (size_t)(hit - p));
            p = hit + dl;
            n++;
        } else {
            pxr_explode_push(out, p, (size_t)(end - p));
            p = end;
            n++;
        }
    }
    return out;
}

long long pxr_array_val_tag_at(void *p, long long i) {
    pxr_array *a = (pxr_array *)p;
    if (i < 0 || (size_t)i >= a->len) {
        return 0;
    }
    return a->ents[(size_t)i].vt;
}

/* ------------------------------- more string builtins (M2) -----------------
 *
 * Byte-exact against PHP 8.4. Case helpers operate on ASCII bytes only, like
 * PHP's locale-independent `toupper`/`tolower`.
 */

long long pxr_str_contains(const pxr_str *h, const pxr_str *n) {
    if (n->len == 0) {
        return 1; /* the empty needle is found everywhere */
    }
    if (n->len > h->len) {
        return 0;
    }
    for (size_t i = 0; i + n->len <= h->len; i++) {
        if (memcmp(h->data + i, n->data, n->len) == 0) {
            return 1;
        }
    }
    return 0;
}

long long pxr_str_starts_with(const pxr_str *h, const pxr_str *n) {
    return n->len <= h->len && memcmp(h->data, n->data, n->len) == 0;
}

long long pxr_str_ends_with(const pxr_str *h, const pxr_str *n) {
    return n->len <= h->len &&
           memcmp(h->data + (h->len - n->len), n->data, n->len) == 0;
}

/* PHP `strrev` reverses *bytes* (not characters), so multi-byte sequences are
 * reordered byte-wise. */
pxr_str *pxr_strrev(const pxr_str *s) {
    pxr_str *out = pxr_str_alloc(s->len);
    for (size_t i = 0; i < s->len; i++) {
        out->data[i] = s->data[s->len - 1 - i];
    }
    return out;
}

pxr_str *pxr_ucfirst(const pxr_str *s) {
    pxr_str *out = pxr_str_alloc(s->len);
    memcpy(out->data, s->data, s->len);
    if (s->len) {
        unsigned char c = (unsigned char)out->data[0];
        if (c >= 'a' && c <= 'z') {
            out->data[0] = (char)(c - 32);
        }
    }
    return out;
}

pxr_str *pxr_lcfirst(const pxr_str *s) {
    pxr_str *out = pxr_str_alloc(s->len);
    memcpy(out->data, s->data, s->len);
    if (s->len) {
        unsigned char c = (unsigned char)out->data[0];
        if (c >= 'A' && c <= 'Z') {
            out->data[0] = (char)(c + 32);
        }
    }
    return out;
}

/* PHP `ucwords` default delimiters: the ASCII whitespace set. Only the first
 * byte of each word is uppercased; the rest is untouched. */
static int pxr_ucwords_space(unsigned char c) {
    switch (c) {
        case ' ':
        case '\t':
        case '\n':
        case '\r':
        case '\f':
        case '\v':
            return 1;
        default:
            return 0;
    }
}

pxr_str *pxr_ucwords(const pxr_str *s) {
    pxr_str *out = pxr_str_alloc(s->len);
    memcpy(out->data, s->data, s->len);
    int at_start = 1;
    for (size_t i = 0; i < s->len; i++) {
        unsigned char c = (unsigned char)s->data[i];
        if (at_start && c >= 'a' && c <= 'z') {
            out->data[i] = (char)(c - 32);
        }
        at_start = pxr_ucwords_space(c);
    }
    return out;
}

/* PHP `str_split`: split into chunks of at most `length` bytes. An empty string
 * yields an empty array (PHP 8.2+); `length < 1` is a fatal ValueError. */
void *pxr_str_split(const pxr_str *s, long long length) {
    if (length < 1) {
        fprintf(stderr,
                "Fatal error: Uncaught ValueError: str_split(): Argument #2 "
                "($length) must be greater than 0\n");
        exit(255);
    }
    pxr_array *out = (pxr_array *)pxr_array_new();
    for (size_t i = 0; i < s->len;) {
        size_t n = (size_t)length;
        if (n > s->len - i) {
            n = s->len - i;
        }
        pxr_explode_push(out, s->data + i, n);
        i += n;
    }
    return out;
}

/* `array_values`: drop keys, keep insertion order, reindex 0..n-1. Nested
 * arrays are deep-copied so value semantics hold. */
void *pxr_array_values(void *p) {
    pxr_array *s = (pxr_array *)p;
    pxr_array *d = (pxr_array *)pxr_array_new();
    for (size_t i = 0; i < s->len; i++) {
        int64_t bits = s->ents[i].bits;
        if (s->ents[i].vt == PXR_TAG_ARR && bits) {
            bits = (int64_t)(intptr_t)pxr_array_copy((void *)(intptr_t)bits);
        }
        pxr_array_push(d, s->ents[i].vt, bits);
    }
    return d;
}

/* `array_reverse`: reverse insertion order. With `preserve_keys` false
 * (PHP default) string keys are kept and integer keys are renumbered from 0 in
 * the new order; true keeps every key. */
void *pxr_array_reverse(void *p, long long preserve_keys) {
    pxr_array *s = (pxr_array *)p;
    pxr_array *d = (pxr_array *)pxr_array_new();
    for (size_t i = s->len; i-- > 0;) {
        pxr_ent *e = &s->ents[i];
        int64_t bits = e->bits;
        if (e->vt == PXR_TAG_ARR && bits) {
            bits = (int64_t)(intptr_t)pxr_array_copy((void *)(intptr_t)bits);
        }
        if (preserve_keys) {
            if (e->ks != NULL) {
                pxr_array_set(d, PXR_TAG_STR, 0, e->ks, e->vt, bits);
            } else {
                pxr_array_set(d, PXR_TAG_INT, e->ki, NULL, e->vt, bits);
            }
        } else if (e->ks != NULL) {
            pxr_array_set(d, PXR_TAG_STR, 0, e->ks, e->vt, bits);
        } else {
            pxr_array_push(d, e->vt, bits);
        }
    }
    return d;
}

/* ------------------------------------------------- M2 array builtins ----
 *
 * `array_merge`, `array_slice`, `array_pad` and `array_fill` all share the same
 * value semantics as `array_values`/`array_reverse`: nested arrays are
 * deep-copied, scalars and (immutable) strings are shared.
 */

/* Copy an array element payload, deep-copying a nested array if needed. */
static int64_t pxr_val_bits_copy(long long vt, int64_t bits) {
    if (vt == PXR_TAG_ARR && bits) {
        return (int64_t)(intptr_t)pxr_array_copy((void *)(intptr_t)bits);
    }
    return bits;
}

/* Append every entry of `s` onto `d`: integer keys are renumbered sequentially
 * (PHP's `$d[] = ...`), string keys are kept and overwrite in place. This is
 * the shared primitive behind `array_merge`, `array_slice` and `array_pad`. */
static void pxr_array_append(pxr_array *d, const pxr_array *s) {
    for (size_t i = 0; i < s->len; i++) {
        const pxr_ent *e = &s->ents[i];
        int64_t bits = pxr_val_bits_copy(e->vt, e->bits);
        if (e->ks != NULL) {
            pxr_array_set(d, PXR_TAG_STR, 0, e->ks, e->vt, bits);
        } else {
            pxr_array_push(d, e->vt, bits);
        }
    }
}

/* `array_merge($a, $b)`: appends `$b` onto a fresh copy of `$a`. The compiler
 * folds a variadic call into a chain, starting from an empty array so that even
 * a single argument is renumbered (PHP reindexes integer keys). */
void *pxr_array_merge(void *pa, void *pb) {
    pxr_array *d = (pxr_array *)pxr_array_new();
    pxr_array_append(d, (const pxr_array *)pa);
    pxr_array_append(d, (const pxr_array *)pb);
    return d;
}

/* `array_slice($a, $offset, $length, $preserve_keys)`. `$has_length` is 0 when
 * the caller omitted `$length` (slice to the end). A negative offset counts
 * from the end; a negative length stops that many elements before the end.
 * String keys are always kept; integer keys are renumbered unless
 * `$preserve_keys` (PHP's default is false). */
void *pxr_array_slice(void *p, long long offset, long long length,
                      long long has_length, long long preserve_keys) {
    pxr_array *s = (pxr_array *)p;
    long long n = (long long)s->len;
    if (offset < 0) {
        offset += n;
        if (offset < 0) {
            offset = 0;
        }
    } else if (offset > n) {
        offset = n;
    }
    long long end;
    if (!has_length) {
        end = n;
    } else if (length < 0) {
        end = n + length;
        if (end < offset) {
            end = offset;
        }
    } else {
        end = offset + length;
        if (end > n) {
            end = n;
        }
    }
    pxr_array *d = (pxr_array *)pxr_array_new();
    for (long long i = offset; i < end; i++) {
        const pxr_ent *e = &s->ents[(size_t)i];
        int64_t bits = pxr_val_bits_copy(e->vt, e->bits);
        if (e->ks != NULL) {
            pxr_array_set(d, PXR_TAG_STR, 0, e->ks, e->vt, bits);
        } else if (preserve_keys) {
            pxr_array_set(d, PXR_TAG_INT, e->ki, NULL, e->vt, bits);
        } else {
            pxr_array_push(d, e->vt, bits);
        }
    }
    return d;
}

/* `array_pad($a, $size, $value)`: pad to `|$size|` elements. With no padding
 * needed PHP returns the array unchanged (keys preserved); otherwise integer
 * keys are renumbered and the padding is appended (positive size) or prepended
 * (negative size) with fresh integer keys. */
void *pxr_array_pad(void *p, long long size, long long vtag, long long vbits) {
    pxr_array *s = (pxr_array *)p;
    long long n = (long long)s->len;
    long long target = size < 0 ? -size : size;
    if (target <= n) {
        return pxr_array_copy(p);
    }
    long long pad = target - n;
    pxr_array *d = (pxr_array *)pxr_array_new();
    if (size < 0) {
        for (long long i = 0; i < pad; i++) {
            pxr_array_push(d, vtag, pxr_val_bits_copy(vtag, vbits));
        }
        pxr_array_append(d, s);
    } else {
        pxr_array_append(d, s);
        for (long long i = 0; i < pad; i++) {
            pxr_array_push(d, vtag, pxr_val_bits_copy(vtag, vbits));
        }
    }
    return d;
}

/* `array_fill($start, $count, $value)`: `$count` copies under integer keys
 * `$start .. $start + $count - 1`. A negative count is a fatal ValueError. */
void *pxr_array_fill(long long start, long long count, long long vtag,
                     long long vbits) {
    if (count < 0) {
        fprintf(stderr,
                "Fatal error: Uncaught ValueError: array_fill(): Argument #2 "
                "($count) must be greater than or equal to 0\n");
        exit(255);
    }
    pxr_array *d = (pxr_array *)pxr_array_new();
    for (long long i = 0; i < count; i++) {
        pxr_array_set(d, PXR_TAG_INT, start + i, NULL, vtag,
                      pxr_val_bits_copy(vtag, vbits));
    }
    return d;
}

/* `str_pad($s, $length, $pad, $type)`: pad to `$length` bytes. Each side cycles
 * the pad string from its start (PHP semantics). An empty pad string or an
 * invalid pad type is a fatal ValueError. */
pxr_str *pxr_str_pad(const pxr_str *s, long long length, const pxr_str *pad,
                     long long type) {
    if (pad->len == 0) {
        fprintf(stderr,
                "Fatal error: Uncaught ValueError: str_pad(): Argument #3 "
                "($pad_string) must not be empty\n");
        exit(255);
    }
    if (type != 0 && type != 1 && type != 2) {
        fprintf(stderr,
                "Fatal error: Uncaught ValueError: str_pad(): Argument #4 "
                "($pad_type) must be STR_PAD_LEFT, STR_PAD_RIGHT, or "
                "STR_PAD_BOTH\n");
        exit(255);
    }
    if (length <= (long long)s->len) {
        pxr_str *out = pxr_str_alloc(s->len);
        memcpy(out->data, s->data, s->len);
        return out;
    }
    long long padn = length - (long long)s->len;
    long long plen = (long long)pad->len;
    long long left = 0, right = 0;
    if (type == 0) {
        left = padn;
    } else if (type == 1) {
        right = padn;
    } else {
        left = padn / 2;
        right = padn - left;
    }
    pxr_str *out = pxr_str_alloc((size_t)((long long)s->len + padn));
    size_t pos = 0;
    for (long long i = 0; i < left; i++) {
        out->data[pos++] = pad->data[(size_t)(i % plen)];
    }
    memcpy(out->data + pos, s->data, s->len);
    pos += s->len;
    for (long long i = 0; i < right; i++) {
        out->data[pos++] = pad->data[(size_t)(i % plen)];
    }
    return out;
}

/* `substr_count($haystack, $needle, $offset, $length)`: number of
 * non-overlapping occurrences of `$needle` in the selected slice. Reproduces
 * PHP's argument checks (empty needle first, then offset, then length) and their
 * exact fatal messages. */
long long pxr_substr_count(const pxr_str *h, const pxr_str *n, long long offset,
                           long long length, long long has_length) {
    if (n->len == 0) {
        fprintf(stderr,
                "Fatal error: Uncaught ValueError: substr_count(): Argument #2 "
                "($needle) must not be empty\n");
        exit(255);
    }
    long long len = (long long)h->len;
    if (offset < 0) {
        offset += len;
        if (offset < 0) {
            fprintf(stderr,
                    "Fatal error: Uncaught ValueError: substr_count(): Argument "
                    "#3 ($offset) must be contained in argument #1 ($haystack)\n");
            exit(255);
        }
    } else if (offset > len) {
        fprintf(stderr,
                "Fatal error: Uncaught ValueError: substr_count(): Argument #3 "
                "($offset) must be contained in argument #1 ($haystack)\n");
        exit(255);
    }
    if (!has_length) {
        length = len - offset;
    } else if (length < 0) {
        length += len - offset;
        if (length < 0) {
            length = 0;
        }
    } else if (length > len - offset) {
        fprintf(stderr,
                "Fatal error: Uncaught ValueError: substr_count(): Argument #4 "
                "($length) must be contained in argument #1 ($haystack)\n");
        exit(255);
    }
    long long count = 0;
    long long nlen = (long long)n->len;
    long long stop = offset + length;
    for (long long i = offset; i + nlen <= stop;) {
        if (memcmp(h->data + i, n->data, (size_t)nlen) == 0) {
            count++;
            i += nlen;
        } else {
            i++;
        }
    }
    return count;
}

/* ---------------------------------------------- escaping & encoding builtins */

/*
 * PHP `addslashes`: put a backslash before `'`, `"` and `\`, and replace a NUL
 * byte with the two characters `\0` (backslash + ASCII zero). Other bytes,
 * including all high bytes, are untouched.
 */
pxr_str *pxr_addslashes(const pxr_str *s) {
    size_t extra = 0;
    for (size_t i = 0; i < s->len; i++) {
        char c = s->data[i];
        if (c == '\'' || c == '"' || c == '\\' || c == '\0') {
            extra++;
        }
    }
    pxr_str *out = pxr_str_alloc(s->len + extra);
    size_t j = 0;
    for (size_t i = 0; i < s->len; i++) {
        char c = s->data[i];
        if (c == '\'' || c == '"' || c == '\\') {
            out->data[j++] = '\\';
            out->data[j++] = c;
        } else if (c == '\0') {
            out->data[j++] = '\\';
            out->data[j++] = '0';
        } else {
            out->data[j++] = c;
        }
    }
    return out;
}

/*
 * PHP `stripslashes`: drop each backslash and emit the byte that follows it,
 * except that `\0` (backslash + ASCII zero) becomes a NUL byte. A trailing
 * backslash is dropped. Matches PHP 8.4 byte-for-byte, including the (slightly
 * surprising) fact that `\n` becomes `n`.
 */
pxr_str *pxr_stripslashes(const pxr_str *s) {
    pxr_str *out = pxr_str_alloc(s->len);
    size_t j = 0;
    for (size_t i = 0; i < s->len; i++) {
        char c = s->data[i];
        if (c == '\\') {
            if (i + 1 >= s->len) {
                break;
            }
            i++;
            char n = s->data[i];
            out->data[j++] = (n == '0') ? '\0' : n;
        } else {
            out->data[j++] = c;
        }
    }
    out->len = j;
    out->data[j] = '\0';
    return out;
}

/* Lowercase hexadecimal, two digits per input byte. */
pxr_str *pxr_bin2hex(const pxr_str *s) {
    static const char hexd[] = "0123456789abcdef";
    pxr_str *out = pxr_str_alloc(s->len * 2);
    for (size_t i = 0; i < s->len; i++) {
        unsigned char b = (unsigned char)s->data[i];
        out->data[i * 2] = hexd[b >> 4];
        out->data[i * 2 + 1] = hexd[b & 0x0f];
    }
    return out;
}

/* Standard RFC 4648 base64, with `=` padding, byte-exact vs PHP. */
pxr_str *pxr_base64_encode(const pxr_str *s) {
    static const char b64[] =
        "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    size_t n = s->len;
    pxr_str *out = pxr_str_alloc(((n + 2) / 3) * 4);
    size_t i = 0;
    size_t j = 0;
    while (i + 3 <= n) {
        uint32_t v = ((uint32_t)(unsigned char)s->data[i] << 16) |
                     ((uint32_t)(unsigned char)s->data[i + 1] << 8) |
                     (uint32_t)(unsigned char)s->data[i + 2];
        out->data[j++] = b64[(v >> 18) & 63];
        out->data[j++] = b64[(v >> 12) & 63];
        out->data[j++] = b64[(v >> 6) & 63];
        out->data[j++] = b64[v & 63];
        i += 3;
    }
    if (n - i == 1) {
        uint32_t v = (uint32_t)(unsigned char)s->data[i] << 16;
        out->data[j++] = b64[(v >> 18) & 63];
        out->data[j++] = b64[(v >> 12) & 63];
        out->data[j++] = '=';
        out->data[j++] = '=';
    } else if (n - i == 2) {
        uint32_t v = ((uint32_t)(unsigned char)s->data[i] << 16) |
                     ((uint32_t)(unsigned char)s->data[i + 1] << 8);
        out->data[j++] = b64[(v >> 18) & 63];
        out->data[j++] = b64[(v >> 12) & 63];
        out->data[j++] = b64[(v >> 6) & 63];
        out->data[j++] = '=';
    }
    return out;
}

/*
 * The unreserved set PHP's URL encoders pass through: `A-Z a-z 0-9 - _ .`, plus
 * `~` for `rawurlencode` (RFC 3986). `urlencode` additionally turns a space into
 * `+` and percent-encodes `~`; everything else becomes uppercase `%XX`.
 */
static int pxr_url_unreserved(unsigned char c, int keep_tilde) {
    if ((c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z') ||
        (c >= '0' && c <= '9')) {
        return 1;
    }
    if (c == '-' || c == '_' || c == '.') {
        return 1;
    }
    return c == '~' && keep_tilde;
}

static pxr_str *pxr_url_encode(const pxr_str *s, int keep_tilde,
                               int space_as_plus) {
    static const char hexu[] = "0123456789ABCDEF";
    size_t outlen = 0;
    for (size_t i = 0; i < s->len; i++) {
        unsigned char c = (unsigned char)s->data[i];
        if (pxr_url_unreserved(c, keep_tilde)) {
            outlen++;
        } else if (space_as_plus && c == ' ') {
            outlen++;
        } else {
            outlen += 3;
        }
    }
    pxr_str *out = pxr_str_alloc(outlen);
    size_t j = 0;
    for (size_t i = 0; i < s->len; i++) {
        unsigned char c = (unsigned char)s->data[i];
        if (pxr_url_unreserved(c, keep_tilde)) {
            out->data[j++] = (char)c;
        } else if (space_as_plus && c == ' ') {
            out->data[j++] = '+';
        } else {
            out->data[j++] = '%';
            out->data[j++] = hexu[c >> 4];
            out->data[j++] = hexu[c & 0x0f];
        }
    }
    return out;
}

pxr_str *pxr_urlencode(const pxr_str *s) {
    return pxr_url_encode(s, 0, 1);
}

pxr_str *pxr_rawurlencode(const pxr_str *s) {
    return pxr_url_encode(s, 1, 0);
}

static int pxr_hexval(unsigned char c) {
    if (c >= '0' && c <= '9') {
        return c - '0';
    }
    if (c >= 'a' && c <= 'f') {
        return c - 'a' + 10;
    }
    if (c >= 'A' && c <= 'F') {
        return c - 'A' + 10;
    }
    return -1;
}

/*
 * `%XX` decoding. A `%` not followed by two hex digits is kept literally (PHP
 * leaves the whole incomplete/invalid sequence untouched). `urldecode` also
 * maps `+` to a space; `rawurldecode` leaves `+` alone.
 */
static pxr_str *pxr_url_decode(const pxr_str *s, int plus_as_space) {
    pxr_str *out = pxr_str_alloc(s->len);
    size_t j = 0;
    for (size_t i = 0; i < s->len; i++) {
        unsigned char c = (unsigned char)s->data[i];
        if (plus_as_space && c == '+') {
            out->data[j++] = ' ';
        } else if (c == '%' && i + 2 < s->len) {
            int hi = pxr_hexval((unsigned char)s->data[i + 1]);
            int lo = pxr_hexval((unsigned char)s->data[i + 2]);
            if (hi >= 0 && lo >= 0) {
                out->data[j++] = (char)((hi << 4) | lo);
                i += 2;
            } else {
                out->data[j++] = '%';
            }
        } else {
            out->data[j++] = (char)c;
        }
    }
    out->len = j;
    out->data[j] = '\0';
    return out;
}

pxr_str *pxr_urldecode(const pxr_str *s) {
    return pxr_url_decode(s, 1);
}

pxr_str *pxr_rawurldecode(const pxr_str *s) {
    return pxr_url_decode(s, 0);
}

/* ---------------------------------------------------- sprintf / printf (M2) */

typedef struct {
    char *data;
    size_t len;
    size_t cap;
} pxr_buf;

static void pxr_buf_init(pxr_buf *b, size_t cap) {
    if (cap < 64) cap = 64;
    b->data = (char *)malloc(cap);
    b->len = 0;
    b->cap = cap;
}

static void pxr_buf_append(pxr_buf *b, const char *s, size_t n) {
    if (!n) return;
    if (b->len + n > b->cap) {
        size_t ncap = b->cap * 2;
        while (b->len + n > ncap) ncap *= 2;
        b->data = (char *)realloc(b->data, ncap);
        b->cap = ncap;
    }
    memcpy(b->data + b->len, s, n);
    b->len += n;
}

static void pxr_buf_append_byte(pxr_buf *b, char c) {
    pxr_buf_append(b, &c, 1);
}

static void pxr_buf_append_pad(pxr_buf *b, char pad, size_t count) {
    for (size_t i = 0; i < count; i++) {
        pxr_buf_append_byte(b, pad);
    }
}

static pxr_str *pxr_arg_to_str(const pxr_array *args, size_t idx) {
    if (idx >= args->len) return pxr_str_alloc(0);
    return pxr_val_to_str(args->ents[idx].vt, args->ents[idx].bits);
}

static int64_t pxr_arg_to_i64(const pxr_array *args, size_t idx) {
    if (idx >= args->len) return 0;
    int64_t vt = args->ents[idx].vt;
    int64_t bits = args->ents[idx].bits;
    switch (vt) {
        case PXR_TAG_INT:
        case PXR_TAG_BOOL:
            return bits;
        case PXR_TAG_FLOAT:
            return (int64_t)pxr_bits_to_double(bits);
        case PXR_TAG_STR:
            return pxr_str_to_i64((const pxr_str *)(intptr_t)bits);
        default:
            return 0;
    }
}

static double pxr_arg_to_f64(const pxr_array *args, size_t idx) {
    if (idx >= args->len) return 0.0;
    int64_t vt = args->ents[idx].vt;
    int64_t bits = args->ents[idx].bits;
    switch (vt) {
        case PXR_TAG_FLOAT:
            return pxr_bits_to_double(bits);
        case PXR_TAG_INT:
        case PXR_TAG_BOOL:
            return (double)bits;
        case PXR_TAG_STR:
            return pxr_str_to_f64((const pxr_str *)(intptr_t)bits);
        default:
            return 0.0;
    }
}

pxr_str *pxr_sprintf(const pxr_str *fmt, void *vargs) {
    const pxr_array *args = (const pxr_array *)vargs;
    pxr_buf out;
    pxr_buf_init(&out, fmt ? fmt->len + 64 : 64);
    if (!fmt || fmt->len == 0) {
        pxr_str *empty = pxr_str_alloc(0);
        free(out.data);
        return empty;
    }

    size_t currarg = 0;
    long long max_missing_argnum = -1;
    size_t i = 0;

    while (i < fmt->len) {
        if (fmt->data[i] != '%') {
            pxr_buf_append_byte(&out, fmt->data[i]);
            i++;
            continue;
        }
        i++; /* skip '%' */
        if (i >= fmt->len) {
            if (max_missing_argnum >= 0) {
                fprintf(stderr,
                        "Fatal error: Uncaught ArgumentCountError: %lld arguments are required, %zu given\n",
                        max_missing_argnum + 2, args->len + 1);
                exit(255);
            }
            if (currarg >= args->len) {
                fprintf(stderr,
                        "Fatal error: Uncaught ArgumentCountError: %zu arguments are required, %zu given\n",
                        currarg + 2, args->len + 1);
                exit(255);
            }
            fprintf(stderr,
                    "Fatal error: Uncaught ValueError: Missing format specifier at end of string\n");
            exit(255);
        }
        if (fmt->data[i] == '%') {
            pxr_buf_append_byte(&out, '%');
            i++;
            continue;
        }

        long long argnum = -1;
        char padding = ' ';
        int align_left = 0;
        int always_sign = 0;
        int width = 0;
        int precision = -1;

        /* 1. Positional argnum: digits followed by '$' */
        size_t k = i;
        while (k < fmt->len && fmt->data[k] >= '0' && fmt->data[k] <= '9') {
            k++;
        }
        if (k < fmt->len && fmt->data[k] == '$' && k > i) {
            long long n = 0;
            for (size_t d = i; d < k; d++) {
                n = n * 10 + (fmt->data[d] - '0');
            }
            if (n <= 0 || n > 2147483647) {
                fprintf(stderr,
                        "Fatal error: Uncaught ValueError: Argument number specifier must be greater than zero and less than 2147483647\n");
                exit(255);
            }
            argnum = n - 1;
            i = k + 1;
        }

        /* 2. Modifiers: ' ', '0', '-', '+', '\'' */
        while (i < fmt->len) {
            char c = fmt->data[i];
            if (c == ' ' || c == '0') {
                padding = c;
                i++;
            } else if (c == '-') {
                align_left = 1;
                i++;
            } else if (c == '+') {
                always_sign = 1;
                i++;
            } else if (c == '\'') {
                if (i + 1 < fmt->len) {
                    padding = fmt->data[i + 1];
                    i += 2;
                } else {
                    fprintf(stderr,
                            "Fatal error: Uncaught ValueError: Missing padding character\n");
                    exit(255);
                }
            } else {
                break;
            }
        }

        /* 3. Width: digits */
        if (i < fmt->len && fmt->data[i] >= '0' && fmt->data[i] <= '9') {
            long long w = 0;
            while (i < fmt->len && fmt->data[i] >= '0' && fmt->data[i] <= '9') {
                w = w * 10 + (fmt->data[i] - '0');
                i++;
            }
            width = (int)w;
        }

        /* 4. Precision: '.' followed by optional digits */
        if (i < fmt->len && fmt->data[i] == '.') {
            i++;
            long long p = 0;
            while (i < fmt->len && fmt->data[i] >= '0' && fmt->data[i] <= '9') {
                p = p * 10 + (fmt->data[i] - '0');
                i++;
            }
            precision = (int)p;
        }

        /* 5. Optional 'l' */
        if (i < fmt->len && fmt->data[i] == 'l') {
            i++;
        }

        if (argnum < 0) {
            argnum = (long long)currarg;
            currarg++;
        }

        if (argnum >= (long long)args->len) {
            if (argnum > max_missing_argnum) {
                max_missing_argnum = argnum;
            }
            if (i < fmt->len) {
                i++;
            }
            continue;
        }

        if (i >= fmt->len) {
            fprintf(stderr,
                    "Fatal error: Uncaught ValueError: Missing format specifier at end of string\n");
            exit(255);
        }

        char spec = fmt->data[i];
        i++;

        char num_buf[128];
        const char *raw_buf = NULL;
        size_t raw_len = 0;

        switch (spec) {
            case 's': {
                pxr_str *s = pxr_arg_to_str(args, (size_t)argnum);
                size_t slen = s->len;
                if (precision >= 0 && (size_t)precision < slen) {
                    slen = (size_t)precision;
                }
                raw_buf = s->data;
                raw_len = slen;
                break;
            }
            case 'c': {
                num_buf[0] = (char)pxr_arg_to_i64(args, (size_t)argnum);
                raw_buf = num_buf;
                raw_len = 1;
                break;
            }
            case 'd':
            case 'i': {
                int64_t v = pxr_arg_to_i64(args, (size_t)argnum);
                snprintf(num_buf, sizeof(num_buf), always_sign ? "%+lld" : "%lld", (long long)v);
                raw_buf = num_buf;
                raw_len = strlen(num_buf);
                break;
            }
            case 'u': {
                uint64_t v = (uint64_t)pxr_arg_to_i64(args, (size_t)argnum);
                snprintf(num_buf, sizeof(num_buf), "%llu", (unsigned long long)v);
                raw_buf = num_buf;
                raw_len = strlen(num_buf);
                break;
            }
            case 'x': {
                uint64_t v = (uint64_t)pxr_arg_to_i64(args, (size_t)argnum);
                snprintf(num_buf, sizeof(num_buf), "%llx", (unsigned long long)v);
                raw_buf = num_buf;
                raw_len = strlen(num_buf);
                break;
            }
            case 'X': {
                uint64_t v = (uint64_t)pxr_arg_to_i64(args, (size_t)argnum);
                snprintf(num_buf, sizeof(num_buf), "%llX", (unsigned long long)v);
                raw_buf = num_buf;
                raw_len = strlen(num_buf);
                break;
            }
            case 'o': {
                uint64_t v = (uint64_t)pxr_arg_to_i64(args, (size_t)argnum);
                snprintf(num_buf, sizeof(num_buf), "%llo", (unsigned long long)v);
                raw_buf = num_buf;
                raw_len = strlen(num_buf);
                break;
            }
            case 'b': {
                uint64_t v = (uint64_t)pxr_arg_to_i64(args, (size_t)argnum);
                if (v == 0) {
                    num_buf[0] = '0';
                    num_buf[1] = '\0';
                    raw_buf = num_buf;
                    raw_len = 1;
                } else {
                    int bpos = 64;
                    num_buf[bpos] = '\0';
                    while (v > 0) {
                        num_buf[--bpos] = (v & 1) ? '1' : '0';
                        v >>= 1;
                    }
                    raw_buf = &num_buf[bpos];
                    raw_len = 64 - bpos;
                }
                break;
            }
            case 'f':
            case 'F': {
                double v = pxr_arg_to_f64(args, (size_t)argnum);
                int p = (precision >= 0) ? precision : 6;
                snprintf(num_buf, sizeof(num_buf), always_sign ? "%+.*f" : "%.*f", p, v);
                raw_buf = num_buf;
                raw_len = strlen(num_buf);
                break;
            }
            case 'e':
            case 'E': {
                double v = pxr_arg_to_f64(args, (size_t)argnum);
                int p = (precision >= 0) ? precision : 6;
                char fspec[16];
                snprintf(fspec, sizeof(fspec), always_sign ? "%%+.*%c" : "%%.*%c", spec);
                snprintf(num_buf, sizeof(num_buf), fspec, p, v);
                raw_buf = num_buf;
                raw_len = strlen(num_buf);
                break;
            }
            case 'g':
            case 'G':
            case 'h':
            case 'H': {
                double v = pxr_arg_to_f64(args, (size_t)argnum);
                int p = (precision >= 0) ? (precision == 0 ? 1 : precision) : 6;
                char target_c = (spec == 'h' ? 'g' : (spec == 'H' ? 'G' : spec));
                char fspec[16];
                snprintf(fspec, sizeof(fspec), always_sign ? "%%+.*%c" : "%%.*%c", target_c);
                snprintf(num_buf, sizeof(num_buf), fspec, p, v);
                raw_buf = num_buf;
                raw_len = strlen(num_buf);
                break;
            }
            default:
                fprintf(stderr,
                        "Fatal error: Uncaught ValueError: Unknown format specifier \"%c\"\n", spec);
                exit(255);
        }

        if ((size_t)width <= raw_len) {
            pxr_buf_append(&out, raw_buf, raw_len);
        } else {
            size_t pad_len = (size_t)width - raw_len;
            if (align_left) {
                char actual_pad = (padding == '0') ? ' ' : padding;
                pxr_buf_append(&out, raw_buf, raw_len);
                pxr_buf_append_pad(&out, actual_pad, pad_len);
            } else {
                if (padding == '0' && (raw_buf[0] == '-' || raw_buf[0] == '+')) {
                    pxr_buf_append_byte(&out, raw_buf[0]);
                    pxr_buf_append_pad(&out, '0', pad_len);
                    pxr_buf_append(&out, raw_buf + 1, raw_len - 1);
                } else {
                    pxr_buf_append_pad(&out, padding, pad_len);
                    pxr_buf_append(&out, raw_buf, raw_len);
                }
            }
        }
    }

    if (max_missing_argnum >= 0) {
        fprintf(stderr,
                "Fatal error: Uncaught ArgumentCountError: %lld arguments are required, %zu given\n",
                max_missing_argnum + 2, args->len + 1);
        exit(255);
    }

    pxr_str *res = pxr_str_alloc(out.len);
    memcpy(res->data, out.data, out.len);
    free(out.data);
    return res;
}

long long pxr_printf(const pxr_str *fmt, void *args) {
    pxr_str *s = pxr_sprintf(fmt, args);
    if (s && s->len > 0) {
        fwrite(s->data, 1, s->len, stdout);
    }
    return s ? (long long)s->len : 0;
}

/*
 * ===========================================================================
 * Object runtime (M3.1): instantiation and property storage.
 * ===========================================================================
 */

typedef struct {
    const pxr_str *class_name;
    size_t num_props;
    int64_t *tags;
    int64_t *bits;
} pxr_object;

void *pxr_object_new(const pxr_str *name, long long num_props) {
    pxr_object *obj = (pxr_object *)pxr_arena_alloc(sizeof(pxr_object));
    obj->class_name = name;
    obj->num_props = (size_t)num_props;
    if (num_props > 0) {
        obj->tags = (int64_t *)pxr_arena_alloc(sizeof(int64_t) * (size_t)num_props);
        obj->bits = (int64_t *)pxr_arena_alloc(sizeof(int64_t) * (size_t)num_props);
        memset(obj->tags, 0, sizeof(int64_t) * (size_t)num_props);
        memset(obj->bits, 0, sizeof(int64_t) * (size_t)num_props);
    } else {
        obj->tags = NULL;
        obj->bits = NULL;
    }
    return obj;
}

long long pxr_object_get_bits(void *p, long long prop_idx) {
    if (!p) {
        fprintf(stderr, "Fatal error: Uncaught Error: Attempt to read property on null\n");
        exit(255);
    }
    pxr_object *obj = (pxr_object *)p;
    if (prop_idx < 0 || (size_t)prop_idx >= obj->num_props) {
        fprintf(stderr, "Fatal error: property index %lld out of bounds\n", prop_idx);
        exit(255);
    }
    return obj->bits[prop_idx];
}

void pxr_object_set_bits(void *p, long long prop_idx, long long tag, long long bits) {
    if (!p) {
        fprintf(stderr, "Fatal error: Uncaught Error: Attempt to assign property on null\n");
        exit(255);
    }
    pxr_object *obj = (pxr_object *)p;
    if (prop_idx < 0 || (size_t)prop_idx >= obj->num_props) {
        fprintf(stderr, "Fatal error: property index %lld out of bounds\n", prop_idx);
        exit(255);
    }
    obj->tags[prop_idx] = tag;
    obj->bits[prop_idx] = bits;
}

