<?php
function describe(string $label = "values", int ...$values): string {
    $text = $label;
    foreach ($values as $key => $value) {
        $text = $text . ":" . $key . "=" . $value;
    }
    return $text;
}
echo describe(), "\n";
echo describe("numbers", 1, 2, 3), "\n";
echo describe(first: 7, label: "named", second: 8), "\n";
$callback = fn(string ...$parts): string => implode(",", $parts);
echo $callback("native", "variadic"), "\n";
