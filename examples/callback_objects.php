<?php
interface IntCallback {
    public function __invoke(int $value): int;
}

class AddOffset implements IntCallback {
    public int $offset;

    public function __construct(int $offset) {
        $this->offset = $offset;
    }

    public function __invoke(int $value): int {
        return $value + $this->offset;
    }
}

function apply(IntCallback $callback, int $value): int {
    return $callback($value);
}

$add = new AddOffset(10);
echo $add(2), ":", apply($add, 3), "\n";
