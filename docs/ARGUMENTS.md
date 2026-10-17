# Native argument binding

Positional calls support required and optional parameters on native functions,
methods, constructors, invokable objects, closures and arrow functions, including
generic specializations and async functions/closures.

```php
function greet(string $name = "world", string $suffix = "!"): string {
    return "hello " . $name . $suffix;
}
echo greet();
echo greet("native", ".");
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

Named arguments, variadic parameters and argument unpacking remain unfinished.
They are explicit errors; optional-argument support does not imply them.
See [M10 status](M10.md) and [the runnable example](../examples/optional_arguments.php).

Reference: [PHP function arguments](https://www.php.net/manual/en/functions.arguments.php).
