<?php
// PXR extension: affine handles and call-scoped aggregate borrows.
class Counter {
    public int $value;
    public function __construct(int $value) { $this->value = $value; }
    public function add(int $amount): void { $this->value += $amount; }
}
function read(&Counter $counter): int { return $counter->value; }
function increment(&mut Counter $counter): void { $counter->value += 1; }
function append(&mut array<int> $values): void { $values[] = 42; }
function forward<T>(owned<T> $value): owned<T> { return move($value); }

$counter: owned<Counter> = new Counter(1);
echo read(&$counter), ":";
increment(&mut $counter);
$counter->add(3);
$next = forward(move($counter));
echo read(&$next), "\n";

$values: owned<array<int>> = [7];
append(&mut $values);
echo count($values), ":", $values[1], "\n";
drop($values);
drop($next);
// echo $counter->value; // E0290: moved value
