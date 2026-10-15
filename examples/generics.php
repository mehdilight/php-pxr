<?php
// PXR generic syntax; compile with pxr rather than the PHP interpreter.
function identity<T>(T $value): T { return $value; }
function singleton<T>(T $value): array<T> { return [$value]; }
function first<T>(array<T> $values, T $fallback): T {
    if (count($values) == 0) { return $fallback; }
    return $values[0];
}
function relay<U>(U $value): U {
    $copy: U = identity<U>($value);
    return $copy;
}
function repeat<T>(T $value, int $times): T {
    if ($times <= 0) { return $value; }
    return repeat<T>($value, $times - 1);
}

echo identity<int>(42) . ':' . identity('hello') . "\n";
echo first(singleton(7), 0) . ':' . first<string>([], 'empty') . "\n";
echo relay('forwarded') . ':' . repeat(9, 3) . "\n";
$values = [1, 2];
$copy = identity($values);
$copy[0] = 99;
echo $values[0] . ':' . $copy[0] . "\n";
$nested: array<array<int>>=[[8]];
$nestedCopy = identity<array<array<int>>>($nested);
echo $nestedCopy[0][0] . ':' . (8 >> 1) . "\n";
$record = identity<mixed>(['number' => 1, 'label' => 'ok']);
if (is_array($record)) { echo $record['label'] . "\n"; }
