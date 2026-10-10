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
| `string` | `pxr_str *` (length‑prefixed, NUL/binary‑safe) | `ptr` |
| `array<T>` | opaque `void *` | `ptr` |

`bool` is `i1` inside generated code and is passed to the runtime as an `i32`
(0/1).

## Memory model

Strings and arrays are immutable‑to‑the‑runtime. Operations that produce a string
(`pxr_str_concat`, `pxr_i64_to_str`, …) or a new array (`pxr_array_new`) allocate
from a **bump arena**:

* `pxr_arena_alloc(n)` — 8‑byte aligned allocation from the current page.
* `pxr_str_alloc(len)` — allocates a `pxr_str` header (`{ size_t len;
  char data[]; }`) plus `len + 1` bytes; `data` is zero‑terminated.

The arena is a linked list of pages (64 KiB each, doubling only for oversized
requests), so growing it never invalidates pointers handed out earlier. This was
upgraded from a single realloc'd block in milestone 1, once arrays could hold
references to strings allocated before them.

There is **no per‑value free** yet. The arena grows for the life of the process
and is reclaimed by the OS at exit. This is a deliberate trade‑off: it is simple,
fast, and cannot dangle. Owned values, reference counting, and (eventually)
ownership/borrow checking are roadmap items. Until then, long‑running programs
that build strings or arrays in a loop can grow memory; this is documented rather
than hidden.

A string is a `pxr_str *`: `len` bytes of `data`, followed by a NUL terminator
that is not part of the value. Embedded `\0` bytes are first‑class, matching PHP
(`strlen("a\0b")` is 3), and the ABI stays a single `ptr` — only the allocation
layout differs from the former bare `const char *`. `data` sits at offset 8,
mirrored by the `{ i64, [N x i8] }` LLVM constant pool entries the backend emits.

## ABI

```c
/* arena */
void    *pxr_arena_alloc(size_t n);
pxr_str *pxr_str_alloc(size_t len);  /* { len, data[..len] } + NUL terminator */

/* printing (no separator, matching PHP `echo`) */
void   pxr_print_i64(long long v);
void   pxr_print_f64(double v);
void   pxr_print_str(const pxr_str *s);   /* fwrite of s->len bytes */
void   pxr_print_bool(int v);        /* 1 -> "1", 0 -> "" */

/* strings — every `pxr_str *` below is NUL/binary-safe */
pxr_str *pxr_str_concat(const pxr_str *a, const pxr_str *b);
pxr_str *pxr_i64_to_str(long long v);
pxr_str *pxr_f64_to_str(double v);   /* PHP zend_gcvt, precision 14 (see below) */
pxr_str *pxr_bool_to_str(int v);
long long pxr_strlen(const pxr_str *s);

/* string -> number (leading numeric prefix, like PHP) */
long long pxr_str_to_i64(const pxr_str *s);
double    pxr_str_to_f64(const pxr_str *s);

/* truthiness (PHP rules; NaN and "0" handled) */
int    pxr_truthy_i64(long long v);
int    pxr_truthy_f64(double v);
int    pxr_truthy_str(const pxr_str *s);

/* string comparison */
int    pxr_str_eq(const pxr_str *a, const pxr_str *b);   /* 1 if equal (bytes) */
int    pxr_str_cmp(const pxr_str *a, const pxr_str *b);  /* byte-wise order */
long long pxr_str_cmp_loose(const pxr_str *a, const pxr_str *b);
       /* PHP `==`/ordering/`<=>`: -1/0/1, numeric-aware for numeric strings */

/* stdlib string builtins (all `pxr_str *`, byte-exact vs PHP 8.4) */
pxr_str *pxr_str_toupper(const pxr_str *s);
pxr_str *pxr_str_tolower(const pxr_str *s);
pxr_str *pxr_str_trim(const pxr_str *s, const pxr_str *mask, long long mode);
       /* mask == NULL -> PHP default " \t\n\r\0\x0B"; mode 0=both, 1=left, 2=right */
pxr_str *pxr_substr(const pxr_str *s, long long off, long long len, long long has_len);
       /* PHP 8 semantics; out-of-range -> "" (never false) */
pxr_str *pxr_str_repeat(const pxr_str *s, long long n);  /* n<0 -> fatal ValueError */
pxr_str *pxr_str_replace(const pxr_str *search, const pxr_str *replace,
                         const pxr_str *subject);
       /* non-overlapping, left-to-right, no rescan of replacements */
pxr_str *pxr_implode(const pxr_str *glue, void *a);      /* (string) each scalar element */
void    *pxr_explode(const pxr_str *sep, const pxr_str *s, long long limit);
       /* -> new array<string>; empty sep -> fatal ValueError */
long long pxr_str_contains(const pxr_str *h, const pxr_str *n);   /* 0/1 */
long long pxr_str_starts_with(const pxr_str *h, const pxr_str *n);
long long pxr_str_ends_with(const pxr_str *h, const pxr_str *n);
pxr_str *pxr_strrev(const pxr_str *s);                /* byte-wise reverse */
pxr_str *pxr_ucfirst(const pxr_str *s);               /* ASCII first byte only */
pxr_str *pxr_lcfirst(const pxr_str *s);
pxr_str *pxr_ucwords(const pxr_str *s);               /* default whitespace delimiters */
void    *pxr_str_split(const pxr_str *s, long long length);
       /* -> array<string>; length<1 -> fatal ValueError; "" -> [] */
void    *pxr_array_values(void *a);                   /* reindexed, keys dropped */
void    *pxr_array_reverse(void *a, long long preserve_keys);
       /* int keys renumbered unless preserve_keys; deep-copies nested arrays */
void    *pxr_array_merge(void *a, void *b);           /* b appended onto a */
       /* int keys renumbered, string keys overwrite in place, deep copies */
void    *pxr_array_slice(void *a, long long offset, long long length,
                         long long has_length, long long preserve_keys);
       /* has_length==0 => to the end; string keys always kept */
void    *pxr_array_pad(void *a, long long size, long long vtag, long long vbits);
       /* pad to |size|; prepend if negative; no-op keeps keys */
void    *pxr_array_fill(long long start, long long count,
                        long long vtag, long long vbits);
       /* count<0 -> fatal ValueError; keys start..start+count-1 */
pxr_str *pxr_str_pad(const pxr_str *s, long long length, const pxr_str *pad,
                     long long type);                /* 0/1/2 = left/right/both */
       /* empty pad or bad type -> fatal ValueError */
long long pxr_substr_count(const pxr_str *h, const pxr_str *n,
                           long long offset, long long length,
                           long long has_length);    /* non-overlapping count */
       /* empty needle first, then offset, then length -> fatal ValueError */
pxr_str *pxr_addslashes(const pxr_str *s);          /* escapes ', ", \, \0 */
pxr_str *pxr_stripslashes(const pxr_str *s);        /* strips \, \0 -> NUL */
pxr_str *pxr_bin2hex(const pxr_str *s);             /* lowercase, 2 chars/byte */
pxr_str *pxr_base64_encode(const pxr_str *s);       /* RFC 4648, = padded */
pxr_str *pxr_urlencode(const pxr_str *s);           /* space -> +, uppercase hex */
pxr_str *pxr_rawurlencode(const pxr_str *s);        /* RFC 3986, space -> %20 */
pxr_str *pxr_urldecode(const pxr_str *s);           /* + -> space, %XX */
pxr_str *pxr_rawurldecode(const pxr_str *s);        /* %XX, leaves + alone */
pxr_str *pxr_sprintf(const pxr_str *fmt, void *args); /* formatted string creation */
long long pxr_printf(const pxr_str *fmt, void *args); /* formatted print to stdout, returns bytes */
long long pxr_array_val_tag_at(void *a, long long i);    /* element tag by index */

/* math */
long long pxr_abs_i64(long long v);
long long pxr_shl(long long a, long long b);   /* PHP shift rules           */
long long pxr_shr(long long a, long long b);   /* >=64 -> 0/-1; <0 -> fatal */
double    pxr_div(double a, double b);         /* b==0 -> fatal DivisionByZeroError */
long long pxr_mod(long long a, long long b);   /* b==0 -> fatal DivisionByZeroError */
/* float abs uses the LLVM intrinsic target `fabs` from libm */

/* arrays — keys are (tag, int, string); values are (tag, 64-bit payload).
   key/value tags: 0=null, 1=int, 2=float, 3=bool, 4=string, 5=array */
void  *pxr_array_new(void);
void   pxr_array_set(void *a, long long ktag, long long ki, const char *ks,
                     long long vtag, long long bits);
void   pxr_array_push(void *a, long long vtag, long long bits);
long long pxr_array_get_bits(void *a, long long ktag, long long ki, const char *ks);
long long pxr_array_has(void *a, long long ktag, long long ki, const char *ks); /* for `??` */
long long pxr_array_count(void *a);
long long pxr_array_val_bits_at(void *a, long long i);   /* foreach */
void  *pxr_array_copy(void *a);                /* deep copy (value semantics) */
```

Array values cross the ABI as a **type tag plus a 64‑bit payload**: `int` is the
value itself, `bool` is zero‑extended, `float` is its IEEE‑754 bits
(`bitcast`), and `string`/`array` are `ptrtoint`‑ed pointers. The compiler knows
the static element type, so it is the one that packs and unpacks the payload with
`Inst::BitCast`; the runtime only carries the bits. Arrays preserve insertion
order and use linear lookup (a hash index is a roadmap item). `pxr_array_push`
uses PHP's "next key = largest integer key + 1" rule.

PHP array **value semantics** are enforced at compile time: the backend emits a
`pxr_array_copy` (a deep copy, recursing into nested arrays) at every store,
function argument, array-literal element and `foreach` base, so mutation through
one name is never visible through another. Strings are immutable and shared.
Canonical decimal‑integer string keys are normalised to integer keys inside
`pxr_array_set`/`_get_bits`/`_has`, matching PHP (`$a["1"]` ≡ `$a[1]`).

**Float stringification** reproduces PHP's `zend_gcvt` at precision 14:
14 significant digits, scientific notation only when the decimal point lies
outside `[-3, 14]`, mantissas always carry a decimal point (`1e15` → `"1.0E+15"`),
exponents are never zero-padded (`1e-5` → `"1.0E-5"`), integral values print
without a fraction, and `-0.0` prints `"-0"` (constant emission uses an exact
`bitcast i64 {bits} to double` so signs and NaN payloads survive). `pxr_div` /
`pxr_mod` abort with PHP's `DivisionByZeroError` (exit 255) instead of producing
IEEE infinity or UB.

Generated modules `declare` these symbols and call them directly; `clang` resolves
them against the cached runtime object. `main` is emitted with the C‑correct
signature `i32 @main()` so the process exit status is well defined.

## Caching

The runtime object is keyed by an FNV‑1a hash of its source and stored under
`$PXR_CACHE_DIR` (default `~/.cache/pxr`, falling back to
`$HOME/.cache/pxr` and then the temp directory). The first build compiles it; later
builds reuse it. If the cache directory is not writable, PXR compiles the runtime
into the per‑build temporary directory instead.
