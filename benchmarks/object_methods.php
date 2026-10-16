<?php
// Reuse one object: isolate property access and method/GC bookkeeping.
class BenchCounter {
    public int $value = 0;
    public function advance(): int {
        $this->value += 1;
        return $this->value;
    }
}
$counter = new BenchCounter();
$sum = 0;
$i = 0;
while ($i < 2000000) {
    $sum += $counter->advance();
    $i++;
}
echo $sum, ":", $counter->value, "\n";
