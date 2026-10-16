<?php
// Build and look up 5,000 dynamically constructed string keys.
$values = [];
$i = 0;
while ($i < 5000) {
    $values["key-" . $i] = $i;
    $i++;
}
$sum = 0;
$i = 0;
while ($i < 5000) {
    $sum += $values["key-" . $i];
    $i++;
}
echo $sum, "\n";
