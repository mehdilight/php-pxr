# C interoperability syntax proposal — M8

Status: **design proposal, not implemented syntax**. M8 remains planned. This
records the proposed user-facing syntax; it does not make existing PHP extensions
loadable in the native PXR runtime.

## Functions supplied by the platform

Declare C functions using PHP-style signatures inside an external library block:

```php
<?php
extern "C" {
    function abs(i32 $number): i32;
}

echo abs(-42); // 42
```

An unqualified block would resolve symbols through the target's default platform
libraries. The collision between this example and PHP's native `abs` builtin
must be resolved before implementation; a library namespace or explicit symbol
alias is a candidate. The example describes the intended call syntax, not a
final name-resolution rule.

`i32` is an exact signed 32-bit integer. C's `int` matches it on the initial
intended targets; bindings must follow the actual target ABI. Existing PHP `int`
keeps its meaning. Exact-width C types, range checks and argument conversions
need a defined contract; ordinary PHP annotations cannot silently assume a C
layout.

## Explicit libraries

```php
extern "C" from "image_tools" {
    function image_resize(
        cstring $input,
        cstring $output,
        i32 $width,
        i32 $height
    ): i32;
}

$status = image_resize("photo.jpg", "small.jpg", 800, 600);
```

`image_tools` and its functions are illustrative names, not a bundled library.
The proposal names a logical library rather than hard-coding platform-specific
`.so`, `.dylib` or `.dll` filenames. Library search, linker flags, deployment and
static/dynamic linking still need specification.

`cstring` would mean a null-terminated C string. A PHP string argument would
produce a temporary valid for that call, rejecting embedded null bytes. A library
that retains the pointer after returning would need explicit allocation and
lifetime handling; that behavior cannot use a call-scoped temporary. C string
returns require a separate ownership, nullability and decoding contract.

## Readable aliases

Keep the library's original symbol while exposing a PHP-style name:

```php
extern "C" from "image_tools" {
    #[Symbol("image_resize")]
    function resizeImage(
        cstring $input,
        cstring $output,
        i32 $width,
        i32 $height
    ): i32;
}

resizeImage("photo.jpg", "small.jpg", 800, 600);
```

`Symbol` is proposed compiler metadata, not an implemented PHP attribute handler.

## Opaque handles and wrappers

```php
extern "C" from "image_tools" {
    opaque type Image;

    function image_open(cstring $path): ptr<Image>;
    function image_close(ptr<Image> $image): void;
}
```

`opaque type` would identify a foreign type without exposing its fields.
`ptr<Image>` would represent a pointer to it. The example deliberately leaves
allocation failure/null pointers and consuming close semantics unresolved;
implementation must define them before accepting these declarations. A raw
pointer is not automatically a safe `owned<Image>` or a thread-transferable value.

A PXR wrapper could eventually expose ordinary application code:

```php
$image = ImageFile::open("photo.jpg");
$image->resize(800, 600);
$image->save("small.jpg");
```

`ImageFile` is illustrative wrapper code, not a supplied class. Its implementation
would handle ownership, error checking and cleanup explicitly. No new destructor
or exception behavior is implied by this example.

## Implementation sequence and open contracts

Start with library blocks, scalar C types, symbol aliases and opaque pointers.
Then add ownership-aware wrappers and broader data layouts. Before shipping:

* Define target ABI widths, signedness, calling conventions and conversion errors.
* Define library resolution and foreign/native symbol precedence.
* Define null pointers, buffers, pointer access and memory ownership/lifetimes.
* Specify whether calls block an async executor and how callers explicitly offload
  suitable work; C calls must not silently become asynchronous.
* Restrict thread transfer and callbacks so foreign code cannot bypass M7's
  ownership guarantees. User-supplied C code is not automatically memory-safe.
* Specify structs, callbacks and variadic calls separately; the first slice need
  not claim support for them.
* Design exported PXR functions and generated headers, also required by M8.

PHP's `FFI::cdef()` describes declarations in C strings at runtime. This proposal
lets PXR type-check declarations and emit native calls during compilation. It
still requires the C libraries to be available at link/run time as specified.
See [MILESTONES.md](MILESTONES.md) for the milestone scope.
