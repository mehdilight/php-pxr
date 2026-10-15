<?php
namespace Demo\Library;

interface GreeterContract {
    public function greet(): string;
}

class Greeter implements GreeterContract {
    private string $name;
    public static int $created = 0;

    public function __construct(string $name) {
        $this->name = $name;
        self::$created++;
    }

    public function greet(): string {
        return "Hello, " . $this->name;
    }
}

function decorate(string $text): string {
    return "[" . $text . "]";
}
