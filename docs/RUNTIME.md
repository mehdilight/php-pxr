# The PXR runtime

The PXR runtime is the small library every compiled program links against. It is
written in C (`runtime/pxr_rt.c`), embedded into the `pxr` binary at build time,
compiled on demand with the system C compiler, and cached as an object file.

Its purpose is the opposite of PHP's Zend engine: no interpreter, no bytecode, no
refcounting GC, no dynamic dispatch — just enough support for the value
operations the backend emits.

## Value representation

| PXR type | C type | LLVM type |
| --- | --- | --- |
| `int` | `long long` (int64) | `i64` |
| `float` | `double` | `double` |
| `bool` | `int` in the ABI | `i1` (zero‑extended to `i32`/`i64` at calls) |
| `string` | `const char *` (NUL‑terminated) | `ptr` |

`bool` is `i1` inside generated code and is passed to the runtime as an `i32`
(0/1).

## Memory model

Strings are immutable. Operations that produce a string
(`pxr_str_concat`, `pxr_i64_to_str`, …) allocate from a **bump arena**:

* `pxr_arena_alloc(n)` — 8‑byte aligned allocation, doubling growth.
* `pxr_str_alloc(len)` — NUL‑terminated string allocation.

There is **no per‑value free** yet. The arena grows for the life of the process
and is reclaimed by the OS at exit. This is a deliberate milestone‑0 trade‑off:
it is simple, fast, and cannot dangle. Owned values, reference counting, and
(eventually) ownership/borrow checking are roadmap items. Until then, long‑running
programs that build strings in a loop can grow memory; this is documented rather
than hidden.

Strings are NUL‑terminated, so an embedded `\0` byte truncates the value.
Embedded‑NUL‑safe strings (length + pointer) are a roadmap item.

## ABI

```c
/* arena */
void  *pxr_arena_alloc(size_t n);
char  *pxr_str_alloc(size_t len);

/* printing (no separator, matching PHP `echo`) */
void   pxr_print_i64(long long v);
void   pxr_print_f64(double v);
void   pxr_print_str(const char *s);
void   pxr_print_bool(int v);        /* 1 -> "1", 0 -> "" */

/* strings */
char  *pxr_str_concat(const char *a, const char *b);
char  *pxr_i64_to_str(long long v);
char  *pxr_f64_to_str(double v);
char  *pxr_bool_to_str(int v);
long long pxr_strlen(const char *s);

/* string -> number (leading numeric prefix, like PHP) */
long long pxr_str_to_i64(const char *s);
double    pxr_str_to_f64(const char *s);

/* truthiness (PHP rules; NaN and "0" handled) */
int    pxr_truthy_i64(long long v);
int    pxr_truthy_f64(double v);
int    pxr_truthy_str(const char *s);

/* string comparison */
int    pxr_str_eq(const char *a, const char *b);   /* 1 if equal */
int    pxr_str_cmp(const char *a, const char *b);  /* strcmp */

/* math */
long long pxr_abs_i64(long long v);
/* float abs uses the LLVM intrinsic target `fabs` from libm */
```

Generated modules `declare` these symbols and call them directly; `clang` resolves
them against the cached runtime object. `main` is emitted with the C‑correct
signature `i32 @main()` so the process exit status is well defined.

## Caching

The runtime object is keyed by an FNV‑1a hash of its source and stored under
`$PXR_CACHE_DIR` (default `~/.cache/pxr`, falling back to
`$HOME/.cache/pxr` and then the temp directory). The first build compiles it; later
builds reuse it. If the cache directory is not writable, PXR compiles the runtime
into the per‑build temporary directory instead.
