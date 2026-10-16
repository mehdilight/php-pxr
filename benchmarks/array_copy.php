<?php
// Read-only array copies: contrasts eager value copying with PHP's COW.
$values = [];
$i = 0;
while ($i < 1000) {
    $values[] = $i + 1;
    $i++;
}
$sum = 0;
$i = 0;
while ($i < 100000) {
    $copy = $values;
    $sum += $copy[0];
    $i++;
}
echo $sum, "\n";
