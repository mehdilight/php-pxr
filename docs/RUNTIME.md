# The PXR runtime

The PXR runtime is the small library every compiled program links against. It is
written in C (`runtime/pxr_rt.c` plus `runtime/pxr_async.c`), embedded into the
`pxr` binary at build time,
compiled on demand with the system C compiler, and cached as an object file.

The runtime provides the value operations emitted by the native backend,
including strings, arrays, class membership, and tagged mixed operations.
Native-only programs do not need PHP; explicit legacy boundaries start a PHP CLI
worker when executed.

## Value representation

| PXR type | C type | LLVM type |
| --- | --- | --- |
| `int` | `long long` (int64) | `i64` |
| `float` | `double` | `double` |
| `bool` | `int` in the ABI | `i1` (zero‑extended to `i32`/`i64` at calls) |
| `string` | `pxr_str *` (length‑prefixed, NUL/binary‑safe) | `ptr` |
| `array<T>` | opaque `void *` | `ptr` |
| `ClassName` | `pxr_object *` | `ptr` |
| `mixed` | `pxr_mixed *` (`int64_t tag, bits`) | `ptr` |

`bool` is `i1` inside generated code and is passed to the runtime as an `i32`
(0/1).

## Memory model

Strings are immutable; array mutations operate on private value copies.
All runtime-owned memory now uses a **tracked heap**, replacing the page-list
bump arena in M6.1. Private headers link allocations and record their sizes;
returned pointers retain `max_align_t` alignment. Headers stay outside the public
value layout, so generated LLVM and literal string layouts are unchanged.

* `pxr_heap_alloc(n)` allocates a stable block and registers terminal cleanup on
  first use. It checks header-size arithmetic and allocation failure.
* `pxr_heap_resize(p, n)` allocates/copies/releases a private buffer. Only array
  entry buffers and formatting buffers resize; language values keep stable
  addresses. Replaced buffers are released immediately.
* `pxr_heap_free(p)` releases an internally owned block. It must never receive
  literal strings, borrowed interior pointers, or an already released block.
* `pxr_str_alloc(len)` checks header/terminator arithmetic and initializes the
  byte length and trailing zero. Size helpers guard growing buffers and common
  expanding string operations against arithmetic overflow.

The heap releases every remaining block on normal return and runtime errors
that use `exit`, including cyclic objects and PHP handle records. Cleanup is
terminal: roots cannot be used afterward. Aborts, signals, and `_exit` do not run
exit callbacks. Allocation failure continues to abort with `pxr: out of memory`.

**Language values are reclaimed during execution.** After IR optimization, the
compiler computes precise CFG liveness and emits stack root frames at safepoints.
Only live slots, temporary pointers, and pointer-valued integer payloads are
published. Scalar functions proven to have no allocation-capable callees omit
root machinery, and control flow before its first possible allocation omits
polling; allocating paths retain safepoints. A caller protects arguments during its pre-call safepoint, then hands
parameter retention to the callee. Return values remain protected until the
caller resumes. Dead roots are removed across assignment and all control-flow
exits. Explicit `drop` emits a forced collection call.

The collector traces array keys/tagged entries, object names/properties, and
mixed boxes. Class metadata and static storage are explicit global roots. An
allocation index recognizes heap pointers without reading headers before static
literals. An iterative worklist marks reachable graphs; sweep reclaims dead
cycles and buffers. Ordinary collection starts at 1 MiB of payload allocation,
then adapts to live data. These are internal thresholds, not memory limits.

Native stringifier re-entry uses a guarded allocation snapshot for C runtime
locals and a root for its returned string. Callback allocations remain
collectible, and callback garbage is swept before another callback can retain
it. The native PHP handle cache is weak. At collections the worker receives live
object IDs and array lineage IDs, drops dead transfer roots, and runs its cycle
collector. A received object/array schedules a native collection at the next
safepoint to bound retention of large remote values behind small proxies.

[`OWNERSHIP.md`](OWNERSHIP.md) describes the affine source contracts. These use
existing pointer/value layouts; the generated program exposes no unsafe pointer
operations. Shared object fields remain shared, so this is not a thread-transfer
or deterministic resource-destruction guarantee.

A string is a `pxr_str *`: `len` bytes of `data`, followed by a NUL terminator
that is not part of the value. Embedded `\0` bytes are first‑class, matching PHP
(`strlen("a\0b")` is 3), and the ABI stays a single `ptr` — only the allocation
layout differs from the former bare `const char *`. `data` sits at offset 8,
mirrored by the `{ i64, [N x i8] }` LLVM constant pool entries the backend emits.

## ABI

```c
/* strings (heap allocation helpers are private) */
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

/* objects — allocated on the tracked heap, properties indexed by slot, inheritance */
void      *pxr_object_new(long long class_id, const pxr_str *class_name, long long num_props);
void      *pxr_static_storage(long long class_id, long long num_props);
long long  pxr_object_get_bits(void *obj, long long prop_idx);
void       pxr_object_set_bits(void *obj, long long prop_idx, long long tag, long long bits);
void       pxr_class_register(long long class_id, long long super_id);
long long  pxr_object_get_class_id(void *obj);
long long  pxr_instanceof(void *obj, long long target_class_id);
```

### Objects and Inheritance

Objects are stable structures allocated on the tracked heap:

```c
typedef struct {
    int64_t class_id;
    const pxr_str *class_name;
    size_t num_props;
    int64_t *tags;
    int64_t *bits;
} pxr_object;
```

Properties are indexed statically by slot number ($0 \dots N-1$) determined at
compile time; inherited parent properties retain identical slots in subclasses.
Property reads and writes operate on 64‑bit payloads and type tags via
`pxr_object_get_bits` and `pxr_object_set_bits`, sharing the same bitcast
encoding/decoding machinery as array elements. Null object dereferencing aborts
with a PHP-faithful fatal error.

The compiler computes the transitive supertype relation, including all parent
classes and interfaces. Each pair is registered at startup via
`pxr_class_register(class_id, super_id)` into heap-allocated hash buckets keyed
by the concrete class ID. Empty roots require no registration.
Method dispatch lowered directly to concrete symbols `pxr$m$<class>$<method>` when
no overrides exist, and to jump-table dispatchers `pxr$disp$<root>$<method>` checking
`pxr_object_get_class_id` when dynamic overrides exist in the class hierarchy.
The `instanceof` operator calls `pxr_instanceof(obj, target_class_id)`, checking
identity first and then the registered supertype pairs. The same lookup handles
class inheritance, multiple interface inheritance, and inherited implementation
contracts. Calls through interface types use the same dispatcher mechanism.

Static properties reuse the object property-slot ABI through
`pxr_static_storage(class_id, num_props)`, which returns one heap-allocated slot
container per declaring class. The compiler initializes declared defaults once,
before top-level execution. Inherited accesses use the original declaring class;
redeclared properties use the subclass container. Instance construction skips
static defaults. Slot indices currently include static declarations, so instances
can have reserved unused slots. Static methods lower to ordinary mangled
functions without a receiver; qualified calls resolve directly to those symbols.

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

The runtime object is keyed by an FNV‑1a hash of its combined C/PHP source and
selected C compiler path, and stored under
`$PXR_CACHE_DIR` (default `~/.cache/pxr`, falling back to
`$HOME/.cache/pxr` and then the temp directory). The first build compiles it; later
builds reuse it. Builds compile privately and publish cache objects atomically
so concurrent builds cannot observe partial output. If the cache directory is not writable, PXR compiles the runtime
into the per‑build temporary directory instead.

## Tagged mixed values (M4)

`pxr_mixed_box(tag, bits)` allocates a tag/payload pair. Tags 0–6 identify null,
int, float, bool, byte string, array, and object. Storage tag 7 denotes a box
pointer in an array/property slot. Scalars and strings share immutable boxes;
arrays normalize their entries to mixed boxes and deep-copy through storage,
arguments, returns, foreach, constructors, and array transforms. Objects retain
reference identity. Array copies carry comparison lineage until mutation,
preserving PHP copy-on-write equality shortcuts even with NaN elements.

Predicates/conversions inspect tags; guards unbox proven arrays and objects.
Strict equality recurses over ordered array keys/values and uses object identity.
Mixed operators implement numeric-string conversion, integer overflow promotion,
array union, bitwise operations, loose comparison, and ordering. NaNs and
incomparable arrays/objects retain PHP's distinct relational/spaceship behavior.
Generated `pxr_native_object_to_string` dispatches compiled magic methods; opaque
PHP handles stringify and compare in their worker. Native recursive comparisons
and bridge values have bounded nesting rather than risking stack overflow.

## PHP CLI interop (M4)

`pxr_php_include` and `pxr_php_call` start a persistent worker lazily. Its PHP
source (`runtime/pxr_php_worker.php`) is embedded alongside the C runtime and
included in the runtime cache hash. The worker is launched directly with exec,
not through a shell. Requests use stdin; responses use a dedicated descriptor 3
pipe. Ordinary stdout/stderr stay separate, including direct PHP stream writes.
The parent flushes native output before a request; the worker flushes PHP output
before acknowledging it.

The wire format is length-prefixed binary data with little-endian 64-bit
integers/doubles, binary strings, ordered array keys, and object handle IDs.
Arrays cross by value; retained array origins preserve PHP comparison behavior
across round trips. Mutations acquire a new origin. PHP objects use stable IDs while their native handles remain live. The worker
releases dead transfer-table entries during native collection; its own globals
retain their normal PHP references. Request temporaries are cleared after each
response, and new transfer IDs are monotonic rather than reused.
Resources, references, callbacks, and native object transfers fail explicitly.
Uncaught PHP exceptions become native fatal errors; PHP exit codes propagate.
Shutdown closes the request stream and reaps the child process. See
[`INTEROP.md`](INTEROP.md) for the user-facing contract.

## Memory lifecycle verification

`cargo test -p pxr --test runtime_heap` compiles the embedded C runtime with a
small harness. It exercises allocation alignment, buffer growth/shrink/release,
array copies, binary strings, object cycles, static storage, arithmetic overflow,
and exit cleanup after both normal execution and fatal runtime errors. Private
allocation counters verify that cleanup returns the heap to zero blocks/bytes.

For AddressSanitizer and UndefinedBehaviorSanitizer on a supported C toolchain:

```sh
PXR_RUNTIME_SANITIZERS=1 cargo test -p pxr --test runtime_heap --test ownership
```

`PXR_GC_STRESS=1` collects at every generated safepoint. `PXR_GC_STATS=1` prints
collection count, peak payload bytes, and bytes remaining before terminal cleanup
to stderr. These diagnostic environment switches are useful for lifecycle tests:

```sh
PXR_GC_STATS=1 pxr run examples/memory.php
```

The ownership acceptance suite runs optimized/unoptimized binaries, checks
negative ownership programs before lowering, verifies bounded repeated
allocation/cyclic garbage, tests stringifier re-entry, and checks PHP handle
release without losing live identity. Sanitizer mode instruments both the real
runtime and the generated native executable.

## Async foundation

The embedded runtime also includes an internal structured executor with heap
frames, timers, POSIX readiness waits and precise task roots. Free async
functions, affine task locals, timers and an async entry now compile to generated
resumable frames. Owned TCP exchange tasks and consuming cancellation are
available; streaming socket/resource and recoverable error APIs remain pending. See [`ASYNC_RUNTIME.md`](ASYNC_RUNTIME.md) for lifecycle, result, error,
cancellation and descriptor ownership rules. Existing synchronous programs do
not start an executor.
