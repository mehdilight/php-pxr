# Ownership and memory safety (M6)

PXR supports affine owned bindings and call-scoped shared/exclusive borrows.
Existing PHP-style values keep array copying and shared object identity. Their
storage is reclaimed by a precise tracing collector, including unreachable
cycles. Native programs still run without PHP.

## Owned bindings and transfer

```php
function forward<T>(owned<T> $value): owned<T> {
    return move($value);
}

$value: owned<string> = "hello";
$next = forward(move($value));  // inferred owned<string>
echo strlen($next);
drop($next);
```

`owned<T>` is an affine binding: it may be transferred once or discarded.
Initialization/owned arguments/owned returns require a fresh expression or an
explicit `move($owner)`. Fresh expressions include literals, `new`, computed
scalar/string values, and array literals. An ordinary function returning a class
reference does not prove freshness; return `owned<ClassName>` explicitly.

`move($value)` consumes an owned local/parameter. Reading, moving, or dropping it
again is an `E0290` error. Reinitialization makes the binding available again.
An unannotated assignment from an owned result infers an owned binding.
`drop($value)` consumes the binding and requests collection with that binding
removed from the root set. Unused owners are also removed automatically by
compiler liveness; explicit drops are optional. `move` and `drop` are reserved
unqualified intrinsics, not PHP functions.

Owned handles cannot be copied into ordinary variables, mixed storage,
properties, arrays, or ordinary function parameters. Compute an independent
value, move into an owned destination, or borrow for a call. Qualifiers are
allowed on local bindings, parameters, and returns, including generic functions
and methods. Ownership mode is explicit in result annotations; ordinary omitted
return annotations do not infer an owned result.

Control-flow joins require an owner to be available on every reachable path.
Returning paths do not invalidate the surviving path. Loop checks include
repeated conditions, updates, and continue edges. Switching a binding between
shared and owned modes on different paths is rejected. Transfers in switch case
or match condition expressions require explicit control flow instead.

## Call-scoped borrows

```php
class Counter {
    public int $value;
    public function __construct(int $value) { $this->value = $value; }
}

function read(&Counter $counter): int { return $counter->value; }
function increment(&mut Counter $counter): void { $counter->value += 1; }
function append(&mut array<int> $items): void { $items[] = 42; }

$counter: owned<Counter> = new Counter(1);
echo read(&$counter);
increment(&mut $counter);
$items: owned<array<int>> = [];
append(&mut $items);  // modifies the original array, without an argument copy
```

Borrow expressions must be direct call arguments naming an owned local/parameter
or an already borrowed parameter. Several shared borrows may coexist. An
exclusive borrow cannot overlap another borrow, read, move, or drop of the same
binding during argument evaluation or the call. Nested argument calls obey the
same rule. Reborrowing a shared parameter cannot upgrade it to exclusive.
A method call on an owner/exclusive borrower also holds an exclusive receiver
loan while its arguments are evaluated.

`&T` permits reads and rejects mutation through that binding. `&mut T` permits
object-property and array-element mutation. Borrowed bindings cannot be rebound;
these are aggregate views, not PHP variable aliases. Exclusive borrows require
an object or array type, including `array<T>` and generic classes; `&mut int`,
`&mut string`, `&mut mixed`, and unresolved bare generic parameters are rejected.
Shared scalar/string views may return an ordinary scalar/string value.

Borrowed arrays are invariant: no numeric widening, element reboxing, or
`array<Child>` to `array<Parent>` mutable conversion. Class/interface upcasts
preserve their concrete generic ancestor arguments and stable object layout.
Shared borrow method calls are rejected because ordinary PHP methods can mutate;
use a free function to read fields. Potentially mutating implicit object
stringification/comparisons are rejected through shared borrows too, including
stringification of an owner while a shared argument loan is active. Methods may be called through an owner or
exclusive borrower. Explicitly borrowing `$this` in an ordinary method is not
supported; pass the owner to a free borrowing function at the call site.

Every borrow ends when its call returns. Borrow values cannot be stored in
locals, fields, arrays, or returned as reference-valued results. There are no
reference-valued locals or named lifetime parameters in this M6 subset. These
restrictions prove that a call borrow cannot outlive its owner, without lifetime
annotations. This syntax is a PXR extension and is unrelated to PHP's `T &$x`
reference parameters or `&$alias = $variable` aliases.

## Objects, generics, and contracts

An owned object owns its outer handle; ordinary property types retain their
existing value/shared semantics and independent managed lifetimes. Ownership is
not a claim that every reachable object is recursively unique. Shared child
objects can still be shared elsewhere. M7.2 derives Send only for the documented
value-field subset, rejecting shared nested objects and validating dynamic
payloads before copying. See [PARALLEL.md](PARALLEL.md).

Classes freshly constructed for owned destinations are audited, including their
instance methods and ancestors: `$this` cannot escape into shared storage or
reference-valued returns. This prevents a constructor or helper method from
silently duplicating the owned receiver handle. Owning such a class can therefore
reject an otherwise valid PHP-style alias-returning method, even if that method
is unused. Ordinary shared instances of other classes keep their PHP behavior.

Ownership modes survive source checking across modules, namespace aliases,
traits, generics, interfaces, inheritance, and virtual calls. Overrides and
interface implementations preserve parameter/result modes. A validated argument
marker tells lowering when array transfer/borrowing must avoid a PHP argument
copy; ordinary array value copies share backing storage until mutation. The
qualifiers otherwise erase to existing native value representations.
Generic ownership operations are checked even in unused template bodies. A final
check against concrete specialized types prevents ordinary coercion wrappers
from changing the storage representation of a borrow.

Owned fields, nested ownership qualifiers, and ownership-qualified container type
arguments are rejected. For example, `owned<Box<int>>` works;
`array<owned<int>>` and an `owned<int>` property do not. Owner-aware method calls
need a resolvable class type; generic transfer functions can forward an opaque
`owned<T>` without inspecting it. Borrowed `mixed` reads may require explicit
scalar conversions rather than relying on narrowing to erase their borrow mode.

## Reclamation and guarantees

The compiler computes backwards CFG liveness after optimization and publishes
precise pointer roots at safepoints. Roots include live slots, SSA temporaries,
and pointers packed into integer payloads. Callers hand argument retention to
callees before the callee can collect. Return values stay protected until the
caller can publish them. Dead roots disappear across assignment, early return,
break/continue, and normal completion; `drop` forces a collection request.

The collector traces tagged array elements/keys, object properties/class names,
and mixed boxes; static storage and class metadata are explicit global roots.
It uses an iterative mark worklist, so graph depth does not consume C call stack.
Unreachable cycles are swept as ordinary garbage. Native `__toString` re-entry
protects the surrounding runtime's allocation snapshot and returned value while
allowing newly allocated callback garbage to be collected. PHP transfer tables
release unreachable object/array handles; PHP globals keep their own references.

Ordinary collection is threshold-based, so discarded values can remain until a
later safepoint. Memory is bounded by live values, runtime temporaries, and
collection headroom; it does not grow with the number of completed allocation
iterations. This is memory management, not deterministic resource destruction:
native destructors, FFI resources, async cancellation, and thread safety are not
implemented by M6. Native `__destruct` declarations fail with `E0290` rather than
being silently ignored; explicit cleanup functions remain ordinary calls. Fatal `exit` paths release the remaining heap; aborts/signals
rely on operating-system teardown. Arbitrary unsafe pointers are not exposed by
the language.

See [`RUNTIME.md`](RUNTIME.md) for implementation details and test commands.
`examples/ownership.php` demonstrates the syntax; `examples/memory.php` demonstrates
reclamation of ordinary shared cycles.
