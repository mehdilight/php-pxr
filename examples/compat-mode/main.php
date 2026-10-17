<?php
// Run natively with: pxr run --compat examples/compat-mode/main.php
function identity($value) { return $value; }
function increment(int $value): int { return $value + 1; }
function values(array $items): array { return $items; }

$value = 7;
echo increment(identity($value)), "\n";
$value = "text";
echo strtoupper($value), "\n";
$value = ["first" => 1, "second" => "two"];
foreach (values($value) as $key => $item) {
    echo $key, "=", $item, ";";
}
echo "\n";

$callback = fn($input) => $input;
echo $callback("native"), "\n";
