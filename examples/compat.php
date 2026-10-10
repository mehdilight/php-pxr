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

// --- milestone 1: arrays and control flow ---------------------------------

$fibs = [1, 1, 2, 3, 5, 8];
$total = 0;
foreach ($fibs as $v) {
    $total += $v;
}
$fibs[] = 13;
echo "total=" . $total . " count=" . count($fibs) . "\n";

$counts = ["a" => 1, "b" => 2];
$counts["c"] = 3;
echo "abc=" . ($counts["a"] + $counts["b"] + $counts["c"]) . "\n";

for ($i = 0; $i < 3; $i++) {
    echo "i=" . $i . " ";
}
echo "\n";

$n = 4;
$fact = 1;
while ($n > 1) {
    $fact *= $n;
    $n--;
}
echo "4!=" . $fact . "\n";

$j = 0;
do {
    $j++;
} while ($j < 3);
echo "j=" . $j . "\n";

switch (2) {
    case 1:
        echo "one";
        break;
    case 2:
        echo "two";
        break;
    default:
        echo "many";
}
echo "\n";

$r = match (3) {
    1, 2 => "low",
    3, 4 => "mid",
    default => "high",
};
echo "match=" . $r . "\n";

echo "sp=" . (7 <=> 3) . "," . (7 <=> 7) . "," . (3 <=> 7) . "\n";
echo "tern=" . (3 > 2 ? "gt" : "le") . "\n";
echo "bits=" . (6 & 3) . "," . (6 | 1) . "," . (6 ^ 1) . "," . (1 << 3) . "," . (16 >> 2) . "\n";
