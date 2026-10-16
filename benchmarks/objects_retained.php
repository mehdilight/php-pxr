<?php
// Keep every object alive through array growth and collection, then read it.
class BenchRetainedPoint {
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
$points = [];
$i = 0;
while ($i < 100000) {
    $points[] = new BenchRetainedPoint($i, 1);
    $i++;
}
$sum = 0;
foreach ($points as $point) {
    $sum += $point->total();
}
echo count($points), ":", $sum, "\n";
