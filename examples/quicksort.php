<?php
// Quicksort over an integer array — the milestone 1 acceptance program.
//
// This file is valid PHP 8 AND valid PXR, so it can be cross-checked:
//
//   php examples/quicksort.php
//   pxr run examples/quicksort.php
//
// The sort is an iterative (explicit-stack) quicksort so that it works without
// function recursion over the array; the stack is itself an array<int>.
//
// Note: PXR does not treat `$sp++` as an expression (only a statement), so the
// stack pointer is advanced explicitly.

$a = [5, 3, 8, 4, 2, 7, 1, 10, 9, 6];

$stack = [];
$sp = 0;
$stack[$sp] = 0;
$sp = $sp + 1;
$stack[$sp] = count($a) - 1;
$sp = $sp + 1;

while ($sp > 0) {
    $sp = $sp - 1;
    $hi = $stack[$sp];
    $sp = $sp - 1;
    $lo = $stack[$sp];

    if ($lo >= $hi) {
        continue;
    }

    $pivot = $a[$hi];
    $i = $lo - 1;
    for ($j = $lo; $j < $hi; $j++) {
        if ($a[$j] < $pivot) {
            $i = $i + 1;
            $tmp = $a[$i];
            $a[$i] = $a[$j];
            $a[$j] = $tmp;
        }
    }
    $i = $i + 1;
    $tmp = $a[$i];
    $a[$i] = $a[$hi];
    $a[$hi] = $tmp;

    $stack[$sp] = $lo;     $sp = $sp + 1;
    $stack[$sp] = $i - 1;  $sp = $sp + 1;
    $stack[$sp] = $i + 1;  $sp = $sp + 1;
    $stack[$sp] = $hi;     $sp = $sp + 1;
}

foreach ($a as $v) {
    echo $v . " ";
}
echo "\n";
