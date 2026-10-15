<?php
// PXR method type parameters are independent of class type parameters.
interface Identity {
    public function identity<U>(U $value): U;
}
class Box<T> implements Identity {
    private T $value;
    public function __construct(T $value) { $this->value = $value; }
    public function identity<U>(U $value): U { return $value; }
    public function replace<U>(U $value): Box<U> { return new Box<U>($value); }
    public function get(): T { return $this->value; }
    public static function first<U>(array<U> $values): U { return $values[0]; }
}
class Child<T> extends Box<T> {
    public function identity<V>(V $value): V { return parent::identity<V>($value); }
}
function viaInterface(Identity $identity): string {
    return $identity->identity('interface');
}
$box = new Child<int>(42);
echo $box->identity<string>('hello') . ':' . $box->identity(7) . "\n";
echo $box->replace('changed')->get() . ':' . $box->get() . "\n";
echo Box<int>::first<string>(['static']) . ':' . Box<int>::first([9]) . "\n";
echo viaInterface($box) . "\n";
