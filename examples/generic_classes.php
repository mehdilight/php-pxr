<?php
// PXR generic classes keep .php files; run this with pxr.
class Box<T> {
    public T $value;
    public function __construct(T $value) { $this->value = $value; }
    public function get(): T { return $this->value; }
    public function set(T $value): void { $this->value = $value; }
}
function wrap<T>(T $value): Box<T> { return new Box<T>($value); }
function unwrap<T>(Box<T> $box): T { return $box->get(); }

$number = new Box<int>(42);
$word = wrap('hello');
echo unwrap($number) . ':' . unwrap($word) . "\n";
$alias = $number;
$alias->set(7);
echo $number->value . "\n";
$items = new Box<array<int>>([1, 2]);
$copy = $items->get();
$copy[0] = 99;
echo $items->value[0] . ':' . $copy[0] . "\n";
$nested = new Box<Box<int>>($number);
echo unwrap(unwrap($nested)) . "\n";
echo ($number instanceof Box<int>) . ':' . ($word instanceof Box<int>) . "\n";
