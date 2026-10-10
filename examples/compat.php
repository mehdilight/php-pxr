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

// --- PHP-faithful value semantics (arrays, keys, shifts) ------------------

$arrA = [1, 2, 3];
$arrB = $arrA;
$arrB[0] = 99;
echo "copy=" . $arrA[0] . "\n";

$arrE = [4, 5, 6];
foreach ($arrE as $v) {
    $arrE[] = 0;
}
echo "snap=" . count($arrE) . "\n";

$arrInner = [1];
$arrOuter = [$arrInner];
$arrInner[] = 2;
echo "nest=" . count($arrOuter[0]) . "," . count($arrInner) . "\n";

$arrK = [];
$arrK["1"] = 10;
$arrK[1] = 20;
echo "keys=" . count($arrK) . "," . $arrK["1"] . "\n";

$dm = match (true) { default => "fallback", true => "armed" };
echo "mdefault=" . $dm . "\n";

echo "bigshift=" . (1 << 64) . "," . (-1 >> 64) . "," . (2 << 4) . "\n";

// PHP compares two numeric strings by value in loose comparisons.
echo "strlt=" . (("10" < "9") ? "lt" : "ge") . "\n";
echo "streq=" . (("1" == "01") ? "loose" : "ne") . "," . (("1" === "01") ? "strict" : "strictne") . "\n";
echo "strsp=" . ("10" <=> "9") . "\n";

$sw = "1";
switch ($sw) {
    case "01":
        echo "switch=loose\n";
        break;
    default:
        echo "switch=no\n";
}

// PHP float stringification (zend_gcvt, precision 14).
echo "floats=" . 1e14 . "," . 1e16 . "," . 1e-5 . "," . (0.1 + 0.2) . "," .
    (1.0 / 3.0) . "," . 3.14159265358979 . "," . -0.0 . "\n";
