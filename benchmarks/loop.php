<?php
// Benchmark: tight integer accumulation loop. Valid PHP and PXR.
$sum = 0;
$i = 0;
while ($i < 5000000) {
    $sum = $sum + $i;
    $i = $i + 1;
}
echo $sum . "\n";
