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

// PHP strings are byte strings: embedded NULs survive length, equality and
// concatenation.
$nul = "a\0b";
echo "nul=" . strlen($nul) . "," . ($nul === "a\0b" ? "eq" : "ne") . "," .
    strlen($nul . "c") . "\n";

// --- milestone 2: string builtins ------------------------------------------

echo strtoupper("Hello, World 123!") . " " . strtolower("HeLLo 123!") . "\n";
echo strtoupper("héllo") . "\n";
echo "[" . trim("  hello \n\t") . "] [" . trim("xxhelloxy", "xy") . "]\n";
echo "[" . ltrim("  hi  ") . "] [" . rtrim("  hi  ") . "] [" . rtrim("hello///", "/") . "]\n";
echo "[" . trim(" \x0b x \x0b") . "] [" . trim(" \x00 ab \x00 ") . "]\n";
echo substr("abcdef", 1) . " " . substr("abcdef", -4) . " " . substr("abcdef", 2, 3) . " " .
    substr("abcdef", -3, -1) . " " . substr("abcdef", 0, 99) . " " .
    substr("abcdef", 4, -1) . "\n";
echo "[" . substr("a", 2) . "] [" . substr("abcdef", 6) . "] [" . substr("abcdef", 2, 0) . "]\n";
echo str_repeat("ab", 3) . " [" . str_repeat("ab", 0) . "]\n";
echo str_replace("l", "L", "hello world") . "\n";
echo str_replace("ab", "aab", "abab") . " " . str_replace("aa", "x", "aaaa") . "\n";

$pieces = explode(",", "a,b,c");
echo "ex=" . implode("|", $pieces) . " count=" . count($pieces) . "\n";
$pieces = explode(",", "a,b,c", 2);
echo "ex2=" . implode("|", $pieces) . "\n";
$pieces = explode(",", "a,b,c", -1);
echo "ex-1=" . implode("|", $pieces) . "\n";
$pieces = explode(",", ",a,");
echo "exedge=" . implode("|", $pieces) . " count=" . count($pieces) . "\n";
echo "im=" . implode("-", ["x", "y", "z"]) . " " . implode(["a", "b"]) . "\n";
echo "imnum=" . implode("-", [1, 2, 3]) . " " . implode("-", [1.5, 2]) . " " . implode("|", [true, false]) . "\n";

// A word count with the string subset, like a compile-able PHP program.
$words = explode(" ", "the quick brown fox");
$longest = "";
foreach ($words as $w) {
    if (strlen($w) > strlen($longest)) {
        $longest = $w;
    }
}
echo "words=" . count($words) . " longest=" . strtoupper($longest) . "\n";

// --- milestone 2: string predicates, case helpers, array transforms --------

echo "pred=" . (str_contains("hello world", "wor") ? "1" : "0") .
    (str_starts_with("hello", "he") ? "1" : "0") .
    (str_ends_with("hello", "lo") ? "1" : "0") .
    (str_contains("hello", "") ? "1" : "0") .
    (str_contains("hello", "xyz") ? "1" : "0") . "\n";
echo "case=" . ucfirst("hello") . " " . lcfirst("World") . " " . ucwords("the quick fox") . "\n";
echo "srev=" . strrev("abcde") . " [" . strrev("") . "]\n";

$bytes = str_split("abcdef", 2);
echo "split=" . implode("|", $bytes) . " n=" . count($bytes) . "\n";
$single = str_split("xyz");
echo "split1=" . implode("|", $single) . "\n";

$vals = array_values(["x" => 10, "y" => 20, "z" => 30]);
echo "vals=" . $vals[0] . "," . $vals[2] . " n=" . count($vals) . "\n";
$revl = array_reverse([1, 2, 3, 4]);
echo "revl=" . implode(",", $revl) . "\n";
$revk = array_reverse(["a" => 1, "b" => 2, "c" => 3]);
echo "revk=" . $revk["a"] . $revk["c"] . "\n";
$revp = array_reverse([1, 2, 3], true);
echo "revp=" . $revp[2] . $revp[0] . "\n";

// High `\xHH` escapes are raw bytes, not UTF-8 replacement characters.
echo "byte=" . strlen("\xA9\xC3") . " " . (strrev("\xA9\xC3") === "\xC3\xA9" ? "ok" : "bad") . "\n";

// --- milestone 2: array merge/slice/pad/fill, str_pad, substr_count ---------

$merged = array_merge([1, 2, 3], [4, 5]);
echo "merge=" . implode(",", $merged) . " n=" . count($merged) . "\n";
$mkeys = array_merge(["a" => 1, "b" => 2], ["b" => 9, "c" => 3]);
echo "mergek=" . $mkeys["a"] . $mkeys["b"] . $mkeys["c"] . " n=" . count($mkeys) . "\n";
$mre = array_merge([5 => "p", 9 => "q"]);
echo "mergere=" . $mre[0] . $mre[1] . "\n";

$slice1 = array_slice([1, 2, 3, 4, 5], 1);
echo "slice=" . implode(",", $slice1) . "\n";
$slice2 = array_slice([1, 2, 3, 4, 5], -2);
echo "sliceNeg=" . implode(",", $slice2) . "\n";
$slice3 = array_slice([1, 2, 3, 4, 5], 1, -1);
echo "sliceBoth=" . implode(",", $slice3) . "\n";
$slice4 = array_slice([1, 2, 3, 4, 5], 1, 2, true);
echo "sliceKeep=" . $slice4[1] . $slice4[2] . "\n";

$pad1 = array_pad([1, 2, 3], 5, 0);
echo "pad=" . implode(",", $pad1) . "\n";
$pad2 = array_pad([1, 2, 3], -5, 0);
echo "padNeg=" . implode(",", $pad2) . "\n";

$fill1 = array_fill(0, 3, "x");
echo "fill=" . implode(",", $fill1) . "\n";
$fill2 = array_fill(-2, 2, 7);
echo "fillNeg=" . $fill2[-2] . $fill2[-1] . "\n";

$hay = [10, 20, 30];
echo "ake=" . (array_key_exists(1, $hay) ? "1" : "0") .
    (array_key_exists(3, $hay) ? "1" : "0") .
    (array_key_exists("2", $hay) ? "1" : "0") . "\n";

echo "spad=" . "[" . str_pad("abc", 5) . "]" .
    "[" . str_pad("abc", 5, "*") . "]" .
    "[" . str_pad("abc", 6, "12", 2) . "]\n";

echo "scount=" . substr_count("hello world", "o") . "," .
    substr_count("aaaa", "aa") . "," .
    substr_count("hello", "l", -2) . "," .
    substr_count("hello", "l", 0, -2) . "\n";
