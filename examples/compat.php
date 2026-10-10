<?php
// This program is valid PHP 8 AND valid PXR — it uses no PXR-only extensions.
// It exists so the test suite can prove that PXR matches PHP's observable
// behaviour for the shared subset:
//
//   php examples/compat.php
//   pxr run examples/compat.php
//
// Both must print exactly the same bytes.

$n = 0;
$sum = 0;
while ($n < 10) {
    $sum = $sum + $n;
    $n = $n + 1;
}
echo "sum=" . $sum . "\n";

function classify(int $x): string {
    if ($x === 0) {
        return "zero";
    }
    if ($x % 2 === 0) {
        return "even";
    }
    return "odd";
}
echo classify(0) . " " . classify(7) . " " . classify(8) . "\n";

$f = 3.5;
echo "f=" . $f . "\n";
echo "div=" . (7 / 2) . "\n";
echo "mod=" . (7 % 2) . "\n";
echo strval(11) . " " . strlen("abcd") . " " . intval("99") . "\n";
echo abs(-13) . " " . floatval("2.5") . "\n";

$b = true;
echo "b=" . $b . "\n";
echo "lt=" . (3 < 5) . "\n";
echo "eq=" . (3 === 3) . "\n";
