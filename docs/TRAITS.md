# Native traits

M5 supports PHP-style native trait composition for typed properties and methods.
Traits provide reusable code; interfaces provide the nominal capability
contracts used by generic bounds.

```php
trait Label {
    private string $label = 'item';
    public function text(): string { return $this->label; }
}
class Item { use Label; }
echo (new Item())->text();
```

Trait composition happens before ordinary class and generic specialization.
Imported members belong to the consuming class, so visibility, constructors,
`$this`, `self::`, `parent::`, static storage, inheritance, and virtual dispatch
use the existing class rules. A trait can use other traits. Namespace imports,
aliases, and compile-time declaration modules resolve trait names normally.

Class methods take precedence over trait methods, and trait methods take
precedence over inherited methods. Conflicting methods from different traits
require a class implementation. Trait properties participate in the ordinary
typed object layout and array value semantics. Static properties are initialized
for their consuming class using the existing static storage rules.

Traits can supply independent generic methods, generic bounds, and methods with
inferred return types. Their own method parameters retain lexical scope even
when a consuming generic class uses the same parameter name. Bodies are checked
with their consuming class; generic method bodies remain lazy until instantiated.

This is a defined native subset, not full PHP trait coverage. Duplicate trait
properties are rejected even when their declarations are identical. Trait
adaptation blocks (`as`, `insteadof`), abstract trait requirements, and generic
trait declarations/trait type arguments remain unsupported. Use interfaces for
abstract contracts, and class methods for conflict resolution. Invalid uses and
cycles fail with `E0285`; unsupported adaptation syntax fails during parsing.
Traits cannot be constructed or used as value types/generic bounds.
Composition is bounded to 64 nested trait uses and 4,096 expansion steps per
program; exceeding either produces `E0285` rather than unbounded expansion.

[`../examples/traits.php`](../examples/traits.php) is valid PHP and is checked
against the PHP interpreter. Files using independent generic methods or PXR
generic syntax still require PXR.
