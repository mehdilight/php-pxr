# Mixed values and narrowing — M4

`mixed` opts into runtime type tags while concrete declarations keep their
static checks. It can hold null, integers, floats, booleans, byte strings,
arrays, and compiled objects or PHP object handles.

```php
<?php
function boxed(mixed $value): mixed { return $value; }
function describe(mixed $value): string {
    if (is_int($value)) { return 'integer: ' . ($value + 1); }
    if (is_array($value)) { return 'array: ' . count($value); }
    return strval($value);
}
echo describe([1, 2]) . "\n";
$value = boxed(1);
$value = 'now a string';
echo $value . "\n";
```

These declarations work in PHP. PXR also accepts local annotations such as
`$value: mixed = 1;`. Compiled parameters and returns still need annotations;
untyped legacy functions run through the [PHP boundary](INTEROP.md).

## Storage and containers

Scalars, arrays, and objects box automatically at mixed assignments, arguments,
returns, constructor arguments, and instance/static properties. Null literals
already have type mixed. Constant scalar, null, and array property defaults work.

Arrays retain PHP value semantics when copied, passed, returned, nested,
iterated, or transformed. Array payloads normalize their elements to boxes;
objects inside them retain reference identity. Assigning a mixed object shares
that object. Native object string conversion dispatches its public `__toString`.

A concrete `array<T>` can become `array<mixed>`. Heterogeneous literals are
accepted at explicit mixed/array-of-mixed boundaries, including mixed parameters
and returns and arguments to legacy functions. Without a mixed boundary,
inferred arrays keep their homogeneous element checks. Arbitrary recursive
array-type conversions are not implicit.

```php
function record(): mixed { return ['name' => 'item', 'count' => 2]; }
$data = record();
if (is_array($data)) { echo $data['name'] . ':' . $data['count']; }
```

## Operations

* Predicates: `is_int`, `is_float`, `is_bool`, `is_string`, `is_scalar`,
  `is_array`, `is_object`, and `is_null`. Arguments always evaluate.
* Explicit scalar conversions: `intval`, `floatval`, `strval`, `boolval`.
  Echo, print, concatenation, interpolation, truthiness, and boolean operators
  also accept mixed values. Arrays stringify with a warning; objects need
  `__toString` to stringify.
* Arithmetic `+ - * / %`, numeric negation, bitwise `& | ^ ~ << >>`, and
  corresponding compound assignments use runtime tags. Numeric strings work;
  invalid operands, zero divisors, and negative shifts fail at runtime. Integer
  overflow promotes to float. Dynamic integer division preserves an integer
  result when division is exact. Array `+` performs a key-preserving union.
* `===`/`!==` distinguish tags and compare strings by bytes, arrays by ordered
  keys and recursively strict values, and objects by identity. Loose comparison,
  ordering, spaceship, and mixed switch subjects use runtime PHP-style rules.
* `match` compares a mixed subject strictly. Branches can combine mixed and
  concrete values; unrelated concrete branch types still need explicit boxing.
* `??`/`??=` test both missing keys and boxed nulls. False, zero, and empty
  strings retain their values; fallback expressions are lazy. Receivers and keys
  evaluate once. Uninitialized mixed properties can be coalesced safely.

Mixed arithmetic results remain mixed. Assigning one to a concrete destination
requires a guard or explicit conversion; concrete arithmetic keeps its original
static ABI and overflow rules. `**` is outside the current compiled subset.

## Array access

Mixed array reads such as `$value[$key][0]` and `$obj->data['name']` check that
each container is an array and return mixed elements. A key may be int, string,
or mixed with a checked int/string tag. String offsets and scalar/null offset
coercion are not implemented by this array path; a wrong container tag raises
a fatal TypeError. Array guards are still required for iteration and typed array
transforms. Nested writes can create arrays from missing/null parents.

## Narrowing

Scalar predicates narrow local variables and parameters. `is_array` narrows to
`array<mixed>`, enabling count, indexing, append, mutation, iteration, and typed
array transforms. A successful `instanceof` check against a compiled class or
interface enables its properties and methods. `is_object` alone does not prove
a particular class. Null checks constrain surviving paths.

Negation, elseif/else, early returns, short-circuit booleans, ternaries, and
successful strict scalar comparisons propagate facts. Assignments invalidate
previous facts; joins retain only facts valid on every reachable path. Storage
stays boxed even when a read narrows. Numeric increments/decrements require a
proven numeric type.

Loop analysis discards facts for values that may be rebound across iterations;
while/for guards can establish fresh body facts. It remains conservative.
Property and array-element reads do not retain guards: copy a value into a local
before guarding it. Most builtins retain concrete argument requirements.

## Boundaries

Compiled code still excludes union/nullable annotations, resources, PHP reference aliases,
and arbitrary dynamic member names. Concrete types remain non-nullable. Read
potentially missing mixed array keys through `??`; using a direct missing-key
read as a mixed value fails rather than fabricating a box. Exceptions and broad
PHP language/library support are available inside explicitly marked PHP files.

PHP object handles can be passed back to legacy functions, compared, or
stringified; their methods/properties are accessed inside PHP helpers. Native
objects cannot cross into PHP. See [interop rules](INTEROP.md).

Boxes and copies use the tracked heap and precise live-root collection, including
cyclic object graphs. Copying an already boxed array shares its backing storage
until mutation; initial conversion from a typed array still normalizes entries
to mixed boxes. Affine handles and call-scoped borrows are described in
[`OWNERSHIP.md`](OWNERSHIP.md).
Mixed programs that do not execute a PHP boundary run without PHP. See
[`examples/containers.php`](../examples/containers.php),
[`examples/nulls.php`](../examples/nulls.php), and
[`examples/migration/main.php`](../examples/migration/main.php).
