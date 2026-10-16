<?php
// Append and then index 10,000 integers. No COW copies are requested by source.
$values = [];
$i = 0;
while ($i < 10000) {
    $values[] = $i;
    $i++;
}
$sum = 0;
$i = 0;
while ($i < 10000) {
    $sum += $values[$i];
    $i++;
}
echo $sum, "\n";
