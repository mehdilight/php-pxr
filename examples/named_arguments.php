<?php
function pair(int $first = 10, int $second = 20): string {
    return $first . ":" . $second;
}
class NamedMessage {
    public function text(string $prefix = "hello", string $name = "world"): string {
        return $prefix . " " . $name;
    }
}
$callback = fn(int $first = 10, int $second = 20): string => $first . ":" . $second;
echo pair(second: 2, first: 1), "\n";
echo (new NamedMessage())->text(name: "native"), "\n";
echo $callback(second: 8), "\n";
echo substr(string: "abcdef", length: 3, offset: 1), "\n";
