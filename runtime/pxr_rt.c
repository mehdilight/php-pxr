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

void pxr_print_f64(double v) {
    /* PHP-style: integral floats print without a decimal point. */
    if (v == v && v != (double)1.0 / 0.0 && v != (double)-1.0 / 0.0) {
        double r = v < 0 ? -v : v;
        if (r == 0.0 || (r >= 1e-4 && r < 1e15 && v == (double)(long long)v)) {
            printf("%lld", (long long)v);
            return;
        }
    }
    printf("%.14G", v);
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
    /* Keep this in sync with pxr_print_f64. */
    char buf[64];
    int n;
    if (v == v && v != (double)1.0 / 0.0 && v != (double)-1.0 / 0.0) {
        double r = v < 0 ? -v : v;
        if (r == 0.0 || (r >= 1e-4 && r < 1e15 && v == (double)(long long)v)) {
            n = snprintf(buf, sizeof buf, "%lld", (long long)v);
            char *out = pxr_str_alloc((size_t)n);
            memcpy(out, buf, (size_t)n);
            return out;
        }
    }
    n = snprintf(buf, sizeof buf, "%.14G", v);
    char *out = pxr_str_alloc((size_t)n);
    memcpy(out, buf, (size_t)n);
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

/* ------------------------------------------------------------------ math */

long long pxr_abs_i64(long long v) {
    /* Avoid UB for LLONG_MIN. */
    return v < 0 ? (long long)(0u - (unsigned long long)v) : v;
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
    long idx = pxr_array_find(a, ktag, ki, ks);
    return idx < 0 ? 0 : a->ents[idx].bits;
}

/* Returns 1 when the key exists, 0 otherwise (used by `??` / `??=`). */
long long pxr_array_has(void *p, long long ktag, long long ki, const char *ks) {
    pxr_array *a = (pxr_array *)p;
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
