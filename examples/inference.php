<?php
function twice(int $value) { return $value * 2; }
function number(bool $whole) {
    if ($whole) { return 1; }
    return 2.5;
}
function fib(int $n) {
    if ($n < 2) { return $n; }
    return fib($n - 1) + fib($n - 2);
}
function identity<T>(T $value) { return $value; }
class Box<T> {
    private T $value;
    public function __construct(T $value) { $this->value = $value; }
    public function get() { return $this->value; }
    public function replace<U>(U $value) { return new Box<U>($value); }
}
function boxed<T>(T $value) { return new Box<T>($value); }
echo twice(21) . ':' . number(true) . ':' . number(false) . ':' . fib(8) . "\n";
echo identity('hello') . ':' . boxed(7)->get() . "\n";
echo (new Box<int>(42))->replace('method')->get() . "\n";
