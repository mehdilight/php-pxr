<?php
$offset = 10;
$add = function (int $value) use ($offset): int {
    return $value + $offset;
};
$offset = 100;
echo $add(3), ":", $offset, "\n";

$factor = 2;
$multiply = fn(int $value): int => $value * $factor;
echo $multiply(4), "\n";

function makePrefix(string $prefix) {
    return fn(string $value): string => $prefix . $value;
}
$format = makePrefix("kept:");
echo $format("value"), "\n";

class Prefix {
    private string $text;
    public function __construct(string $text) { $this->text = $text; }
    public function formatter() {
        return fn(string $value): string => $this->text . $value;
    }
    public function identity() {
        return static fn(self $value): self => $value;
    }
    public function staticFormatter(string $prefix) {
        return static fn(string $value): string => $prefix . $value;
    }
}
$bound = (new Prefix("bound:"))->formatter();
$unbound = (new Prefix("unused:"))->staticFormatter("static:");
echo $bound("value"), ":", $unbound("value"), "\n";

$identity = (new Prefix("unused:"))->identity();
$copied = $identity(new Prefix("typed:"))->formatter();
echo $copied("value"), "\n";
