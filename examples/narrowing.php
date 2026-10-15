<?php
function describeNarrowed(mixed $value): string {
    if (is_int($value)) {
        $value++;
        $value += 2;
        return "int:" . $value;
    } elseif (is_float($value)) {
        return "float:" . ($value / 2);
    } elseif (is_string($value)) {
        return "length:" . strlen($value);
    } else {
        return $value ? "true" : "false";
    }
}

function lengthAfterGuard(mixed $value): int {
    if (!is_string($value)) { return 0; }
    return strlen($value);
}

function guardedNumber(mixed $value): bool {
    return is_int($value) && $value > 0;
}

echo describeNarrowed(4) . "\n";
echo describeNarrowed(4.5) . "\n";
echo describeNarrowed("hello") . "\n";
echo describeNarrowed(false) . "\n";
echo lengthAfterGuard("abc") . ":" . lengthAfterGuard(1) . "\n";
echo (guardedNumber(5) ? "positive" : "other") . "\n";
echo (guardedNumber("5") ? "positive" : "other") . "\n";
