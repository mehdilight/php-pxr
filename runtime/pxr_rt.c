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

typedef struct {
    char  *base;
    size_t cap;
    size_t used;
} pxr_arena;

static pxr_arena g_arena = {NULL, 0, 0};

static void pxr_oom(void) {
    fputs("pxr: out of memory\n", stderr);
    abort();
}

void *pxr_arena_alloc(size_t n) {
    n = (n + 7u) & ~(size_t)7u; /* 8-byte align */
    if (g_arena.used + n > g_arena.cap) {
        size_t want = g_arena.cap ? g_arena.cap * 2 : (size_t)1 << 16;
        while (want < g_arena.used + n) {
            want *= 2;
        }
        char *nb = (char *)realloc(g_arena.base, want);
        if (!nb) {
            pxr_oom();
        }
        g_arena.base = nb;
        g_arena.cap = want;
    }
    char *p = g_arena.base + g_arena.used;
    g_arena.used += n;
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
