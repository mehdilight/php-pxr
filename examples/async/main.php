<?php
// PXR extension: run with `pxr run examples/async/main.php`.
async function delayedValue(int $value): int {
    echo "start ", $value, "\n";
    await sleepAsync(10);
    echo "finish ", $value, "\n";
    return $value;
}

async function main(): void {
    $first = spawn delayedValue(10);
    $second = spawn delayedValue(20);
    $a = await $first;
    $b = await $second;
    echo "sum: ", $a + $b, "\n";
}
