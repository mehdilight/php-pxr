<?php
// Showcases the scalar type system, inference, widening and the builtin
// functions available in milestone 0.

$name: string = "PXR";
$count: int = 3;
$pi: float = 3.14159;
$running: bool = true;

echo "language: " . $name . "\n";
echo "count:    " . $count . "\n";
echo "pi:       " . $pi . "\n";
echo "running:  " . $running . "\n";

// `int` widens to `float` automatically.
$total: float = $count;      // 3 -> 3.0
echo "total:    " . $total . "\n";

// Integer division uses `/` (always float, like PHP) and `%` (int remainder).
echo "10 / 4 = " . (10 / 4) . "\n";
echo "10 % 4 = " . (10 % 4) . "\n";

// Builtins.
echo "strlen:   " . strlen($name) . "\n";
echo "abs:      " . abs(-7) . "\n";
echo "intval:   " . intval("123abc") . "\n";
echo "strval:   " . strval(42) . "\n";
echo "floatval: " . floatval("2.5") . "\n";

// Logic and comparisons.
$x = 7;
if ($x > 5 && $x < 10) {
    echo "x is between 5 and 10\n";
}

$y = 0;
while ($y < 3) {
    echo "y=" . $y . " ";
    $y = $y + 1;
}
echo "\n";
