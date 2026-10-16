<?php
// Copy a populated array, then overwrite two elements of the copy.
$values = [];
$i = 0;
while ($i < 1000) {
    $values[] = $i + 1;
    $i++;
}
$sum = 0;
$i = 0;
while ($i < 10000) {
    $copy = $values;
    $copy[0] = $i;
    $copy[999] = $i + 1;
    $sum += $copy[0] + $copy[999];
    $i++;
}
echo $sum, ":", $values[0], ":", $values[999], "\n";
