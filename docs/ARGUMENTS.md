# Native argument binding

Positional and named calls support required and optional parameters on native functions,
methods, constructors, invokable objects, closures and arrow functions, including
generic specializations and async functions/closures.

```php
function greet(string $name = "world", string $suffix = "!"): string {
    return "hello " . $name . $suffix;
}
echo greet();
echo greet("native", ".");
echo greet(suffix: ".", name: "native");
```

Default expressions currently support constant scalar/array expressions.
Annotations remain required in strict mode; `--compat` uses mixed for untyped
parameters. A default must satisfy its annotation, including nullable/union and
nested array membership. Failure-literal defaults must be statically provable;
use literal `false`/`true`/`null` rather than computed boolean expressions where
the declared type requires one exact value. Type-dependent generic defaults are checked when the
specialization is available. Missing required arguments, extra arguments,
required parameters after optional ones and invalid defaults produce diagnostics.
New-object and nonconstant default expressions remain rejected with E0311.
Trailing commas work in parameter and argument lists.

Calls preserve source argument evaluation and ordinary ownership/copy-on-write
rules. Omitted arguments initialize inside the called function. Virtual and
interface dispatch use the effective implementation's default, even when it
differs from the base/interface declaration. A default is not evaluated when an
explicit argument is supplied. Fresh default arrays do not share mutable headers
between calls. Overrides cannot make an inherited optional parameter required;
the existing native invariant parameter/type-count contracts still apply. An
explicit virtual `__construct()` call requires compatible constructor parameter
types and optionality across possible targets; `new` and class-qualified parent
constructor calls retain their separate constructor contracts.

Owned and borrowed parameters cannot themselves have optional defaults. Required
owned/borrowed prefix parameters can precede optional shared parameters; normal
move/borrow checks still apply. Async calls retain their existing lazy-task
contract and preserve argument storage across suspension. Native worker/server
callback adapters pass full argument presence through the optional-argument ABI.

Named labels are case-sensitive and may use keyword spellings. Supplied expressions
run in source order, including assignments and awaited arguments; native parameter
positions do not change that order. Array arguments snapshot when evaluated.
Positional arguments must precede named arguments. Unknown/duplicate names,
missing required values and conflicting positional/named assignments are errors.
Native virtual named calls currently require matching parameter names across all
possible overrides; an ambiguous hierarchy produces E0312. Direct construction
and class-qualified calls bind to their selected declaration.

Native builtins accept named operands from their supported PHP 8.4 signatures,
for example `strlen(string: "bytes")`, `substr(offset: 1, string: "abcdef")`
and `array_slice(preserve_keys: true, offset: 1, array: $values)`. Skipped native
length defaults keep their omitted-length behavior. Missing APIs/options are not
added by named binding. Unknown named variadic operands of `sprintf`/`printf` and
`array_merge` are rejected, as in PHP. PXR async/I/O/task intrinsics and explicit
PHP interpreter boundaries retain their positional-call contract for now.

Variadic declarations use PHP syntax: `function collect(int ...$values): int`.
Inside the function, `$values` is an `array<int>`; the annotation describes each
supplied element. Empty packs work, and unknown named arguments retain their
string keys in the variadic array. A label matching the variadic parameter name
also becomes a string key. Fixed and variadic named values can interleave;
evaluation, initialization checks, array snapshots and awaited values follow
source order. This applies to native methods, constructors, closures, generic
specializations and async calls. Overrides preserve the variadic contract.

The variadic parameter must be last and cannot have a default. An optional fixed
prefix is allowed. Affine/borrowed element storage is rejected by the ordinary
container ownership checks; required owned/borrowed fixed prefixes remain usable.
Strict mode requires an element annotation; compatibility mode uses mixed for an
untyped variadic declaration.

Array argument unpacking supports native functions, methods, constructors,
closures and generic/async calls: `pair(...$values)` and
`collect(...$values, extra: 7)`. Integer keys supply positional values in insertion
order; string keys select fixed names or become variadic keys. Multiple unpacked
arrays are allowed. Positional arguments cannot follow unpacking; explicit named
arguments can follow it. A later numeric entry after a named entry, duplicate
supplied names, missing required values and wrong runtime types produce errors.
Defaults still execute inside the effective implementation. Unpacked elements
snapshot before later expressions, including awaits. Generic inference uses
homogeneous array evidence; supply explicit type arguments when evidence is
ambiguous or unavailable.

Unpacking currently requires shared parameter storage. Signatures containing
owned/borrowed parameters retain explicit E0290 diagnostics for unpacked calls.
Native builtins accept dynamic unpacked arrays through checked bridges for their
implemented signatures. Optional lengths retain omitted-length behavior;
`sprintf`/`printf` and `array_merge` reject unknown named variadic entries.
`implode` retains its one-array and separator-plus-array forms. Formatting and
joining retain the native scalar-element contract, and dynamic `abs` returns a
checked `int|float` value. Dynamic array results use mixed element storage.
Unsupported builtin options remain errors. Native I/O/task intrinsics and PHP
interpreter boundaries retain their explicit unpacking restriction.
Traversable/generator unpacking remains outstanding.
See [the named-argument example](../examples/named_arguments.php).
See [M10 status](M10.md) and [the runnable example](../examples/optional_arguments.php).

Reference: [PHP function arguments](https://www.php.net/manual/en/functions.arguments.php).
