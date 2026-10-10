<?php
// Recursive Fibonacci — the canonical "it really compiles" smoke test.

function fib(int $n): int {
    if ($n < 2) {
        return $n;
    }
    return fib($n - 1) + fib($n - 2);
}

$i: int = 0;
while ($i <= 15) {
    echo "fib(" . $i . ") = " . fib($i) . "\n";
    $i = $i + 1;
}
