<?php
// Null participates in mixed storage; concrete declarations remain non-nullable.
function boxedNullable(mixed $value): mixed { return $value; }
function absentValue(): mixed { return null; }
function nullFallback(): string { echo "fallback|"; return "ready"; }

class NullableBox {
    public mixed $value = null;
    public mixed $unset;
    public static mixed $shared = null;
}

$value = absentValue();
echo is_null($value) . ":" . is_scalar($value) . ":" . intval($value) . ":" . floatval($value) . ":" . boolval($value) . ":" . strval($value) . "\n";
echo ($value ?? nullFallback()) . "\n";
$value ??= nullFallback();
$value ??= nullFallback();
echo $value . "\n";

$values = [null, boxedNullable(false), boxedNullable(0), boxedNullable(""), boxedNullable("ok")];
foreach ($values as $v) { echo "[" . ($v ?? "missing") . "]"; }
echo "\n";
$values[0] ??= "filled";
$values[1] ??= "wrong";
$values[9] ??= "new";
echo $values[0] . ":" . boolval($values[1]) . ":" . $values[9] . "\n";
echo match (absentValue()) { null => "null matched", default => "wrong" } . "\n";
echo (null === absentValue()) . ":" . (null !== boxedNullable(0)) . "\n";

$box = new NullableBox();
echo ($box->value ?? "default") . ":" . ($box->unset ?? "unset") . "\n";
$box->value ??= "property";
$box->unset ??= "initialized";
NullableBox::$shared ??= "static";
echo $box->value . ":" . $box->unset . ":" . NullableBox::$shared . "\n";
