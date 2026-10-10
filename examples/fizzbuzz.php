<?php
// FizzBuzz — exercises loops, modulo, string concatenation and branching.

$i: int = 1;
while ($i <= 20) {
    if ($i % 15 === 0) {
        echo "FizzBuzz\n";
    } elseif ($i % 3 === 0) {
        echo "Fizz\n";
    } elseif ($i % 5 === 0) {
        echo "Buzz\n";
    } else {
        echo $i . "\n";
    }
    $i = $i + 1;
}
