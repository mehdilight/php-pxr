<?php
interface Printable { public function text(): string; }
interface Named { public function name(): string; }
class Item implements Printable, Named {
    public function text(): string { return 'item'; }
    public function name(): string { return 'named'; }
}
function describe<T: Printable & Named>(T $value): string {
    return $value->name() . ':' . $value->text();
}
class Holder<T: Printable> {
    private T $value;
    public function __construct(T $value) { $this->value = $value; }
    public function text(): string { return $this->value->text(); }
    public static function describe<U: Printable & Named>(U $value): string { return describe($value); }
}
$item = new Item();
echo describe($item) . ':' . Holder<Item>::describe($item) . "\n";
echo (new Holder<Item>($item))->text() . "\n";
