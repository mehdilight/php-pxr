<?php
function boxed(mixed $value): mixed { return $value; }
function choose(bool $text): mixed {
    if ($text) { return "hello"; }
    return 42;
}
function describe(mixed $value): string {
    if (is_int($value)) { return "int:" . intval($value); }
    if (is_float($value)) { return "float:" . floatval($value); }
    if (is_bool($value)) { return "bool:" . strval($value); }
    return "string:" . strval($value);
}

class Box {
    public mixed $value = 7;
    public static mixed $latest = "start";
    public function __construct(mixed $value) { $this->value = $value; }
    public function get(): mixed { return $this->value; }
}

echo describe(boxed(42)) . "\n";
echo describe(boxed(3.5)) . "\n";
echo describe(boxed(true)) . "\n";
echo describe(choose(true)) . "\n";
$box = new Box("boxed");
echo $box->get() . "\n";
$box->value = false;
echo (boolval($box->get()) ? "true" : "false") . "\n";
Box::$latest = 9;
echo Box::$latest . "\n";
$values = [boxed(1), boxed("two"), boxed(false)];
foreach ($values as $value) { echo describe($value) . "\n"; }
echo (boxed(1) === boxed("1") ? "same" : "different") . "\n";
echo match (boxed("yes")) { "yes" => "matched", default => "other" } . "\n";
