<?php
$offset = 10;
$add = function (int $value) use ($offset): int {
    return $value + $offset;
};
$offset = 100;
echo $add(3), ":", $offset, "\n";

$factor = 2;
$multiply = fn(int $value): int => $value * $factor;
echo $multiply(4), "\n";

function makePrefix(string $prefix) {
    return fn(string $value): string => $prefix . $value;
}
$format = makePrefix("kept:");
echo $format("value"), "\n";
