<?php
function identity<T>() {
    return fn(T $value): T => $value;
}
class Factory<T> {
    private T $seed;
    public function __construct(T $seed) { $this->seed = $seed; }
    public function snapshot() {
        return fn(T $fallback): T => $this->seed;
    }
    public function identity<U>() {
        return static fn(U $value): U => $value;
    }
}
$integer = identity<int>();
$text = identity<string>();
echo $integer(42), ":", $text("text"), "\n";

$factory = new Factory<string>("kept");
$snapshot = $factory->snapshot();
$otherType = $factory->identity<int>();
echo $snapshot("ignored"), ":", $otherType(7), "\n";
