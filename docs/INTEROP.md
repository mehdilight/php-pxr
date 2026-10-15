# Gradual migration through PHP — M4

Compile the typed part of a project and leave unmigrated files in PHP. An
explicit header comment selects the interpreter boundary; failed compilation
never silently falls back to PHP.

`legacy.php`:

```php
<?php
// @pxr-dynamic
function legacyTotal($prices) { return array_sum($prices); }
```

`main.php`:

```php
<?php
require_once __DIR__ . '/legacy.php';
$total = legacyTotal([3, 5, 7]);
echo ($total + 1) . "\n";
```

Run either `php main.php` or `pxr run main.php`. Under PXR the entry point is
native code, the included file runs in PHP, and the call returns a mixed value.
`examples/migration/main.php` demonstrates a small catalog application with
native price totals over a CSV catalog, untyped file I/O, array data, globals, default arguments,
and persistent object handles. Its output is tested against PHP.

## Includes and calls

Put `// @pxr-dynamic` in the file's leading comments, normally immediately after
`<?php`, before declarations or executable code. Declaration-only files without
this marker remain [compile-time modules](MODULES.md).

`include`, `include_once`, `require`, and `require_once` are standalone
statements with literal UTF-8 paths, optionally prefixed by `__DIR__ .` and/or
parenthesized. Marked files can be included conditionally, inside functions or
methods, or after native statements. Runtime once behavior uses PHP's own
canonical include registry, including files included internally by legacy code.
Missing optional includes warn and continue in PHP; missing required literals
are compile errors when the compiler cannot resolve them. Include return values
and native variable path expressions remain outside the compiled grammar.

PXR resolves these paths relative to the containing source file, not the
process working directory or PHP include_path. Deploy the PHP files at their
resolved paths, or rebuild at the destination. Inside a legacy file, PHP
handles its own dependencies, autoloaders, namespaces, and include_path normally.

With a boundary in the program, calls not bound to compiled functions or native
builtins resolve in PHP at runtime, returning mixed. Namespace-to-global fallback
is preserved for legacy calls. Explicitly qualified calls have no fallback.
When a namespaced unqualified call could resolve dynamically, its result is
mixed, including possible predicate overrides; qualify builtin predicates such
as `\is_int` to establish native guard facts. Known compiled calls remain
statically bound.

## Runtime and data

A PHP 8+ CLI worker starts only when a boundary executes. It stays alive until
native process exit, preserving legacy globals, static variables, objects, and
include state. PHP stdout/stderr remain separate from a binary response pipe;
normal output and native output preserve operation order. `PXR_PHP` selects an
executable; otherwise the runtime searches PATH for `php`. No shell is used.
Building, checking, and reporting do not execute legacy files or require PHP.

Null, bool, signed 64-bit integers, doubles (including NaN/infinities), binary
strings, and nested ordered arrays cross by value. PHP objects cross as opaque
handles and retain identity when returned repeatedly or passed back to PHP.
Printing/stringifying a handle uses PHP's `__toString`; comparisons involving
handles remain in PHP. Native objects support mixed storage and guarded member
access, but cannot cross into the worker. Pass plain scalar/array records instead.

By-reference parameters, array references, native callbacks, native object
transfers, and resource returns fail explicitly at the boundary. Nesting is
limited to 128 levels. Use a PHP helper to adapt such APIs into value arguments
and results; keep object mutation inside PHP. Exceptions escaping a call become
native fatal errors; explicit PHP exit codes terminate the native program with
that code. Native exception handling is a later language feature.

The worker retains transferred array snapshots and object handles until exit;
long-running workloads can grow memory, as can the native arena (M6).

The worker is an interpreter boundary with its own globals, locals, stdin,
output-buffer context, and symbol table. Native variables and compiled functions
are not implicitly exported to it. This is CLI interop, not an embedded Zend
engine or a web SAPI, and does not claim transparent framework execution.

## Migration report

```sh
pxr migration-report examples/migration/main.php
pxr migration-report examples/migration/main.php --json
```

The report uses the same dependency graph and type checks as build/check. It
lists each discovered file's native/PHP mode and line count, and each compiled
function's concrete/mixed slot counts, mixed return status, and dynamic call
sites. Nested `array<mixed>` slots count as mixed. Synthetic entry code is shown
as `main`; generated method dispatchers can also appear.

JSON schema version 1 has `checked`, `files`, and `functions`. `checked` describes
native checks, not PHP validation. Invalid native code produces diagnostics and
a nonzero exit status with `checked: false`. PHP source is never executed for the
report; dependencies loaded internally by PHP remain owned by that boundary and
are not enumerated as native graph nodes.
