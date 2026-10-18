<?php
function describe(int $first = 10, int ...$rest): string {
    $text = "first=" . $first;
    foreach ($rest as $key => $value) {
        $text = $text . ":" . $key . "=" . $value;
    }
    return $text;
}
$values = [1, 2, 3];
echo describe(...$values, extra: 4), "\n";
$named = ["extra" => 8, "first" => 7];
echo describe(...$named), "\n";
echo describe(...[]), "\n";
echo substr(...["string" => "abcdef", "offset" => 2]), "\n";
function hold(mixed $value): mixed { return $value; }
$arguments = hold(["%d:%s", 7, "native"]);
echo sprintf(...$arguments), "\n";
$arguments = ["separator" => "-", "array" => [1, 2, 3]];
echo implode(...$arguments), "\n";
$arguments = [-7];
echo abs(...$arguments), "\n";
