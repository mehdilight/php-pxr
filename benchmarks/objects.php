<?php
// Repeated object allocation, property reads and method calls.
class BenchPoint {
    public int $x;
    public int $y;
    public function __construct(int $x, int $y) {
        $this->x = $x;
        $this->y = $y;
    }
    public function total(): int {
        return $this->x + $this->y;
    }
}
$sum = 0;
$i = 0;
while ($i < 2000000) {
    $point = new BenchPoint($i, 1);
    $sum += $point->total();
    $i++;
}
echo $sum, "\n";
