<?php
interface CounterContract {
    public static function next(): int;
}

class Counter implements CounterContract {
    protected static int $count = 0;
    private static string $label = "counter";
    public int $instance = 99;

    public static function next(): int {
        self::$count++;
        return self::$count;
    }

    public static function describe(): string {
        return self::$label . ":" . self::$count;
    }
}

class SharedCounter extends Counter {
    public static function next(): int {
        return parent::next() + 10;
    }
}

class SeparateCounter extends Counter {
    protected static int $count = 100;

    public static function next(): int {
        self::$count += 2;
        return self::$count;
    }
}

class Settings {
    public static string $name = "PXR";
    public static float $ratio = 2;
    public static bool $enabled = true;
    public static $values = [1, 2];
}

echo Counter::next() . "\n";
echo SharedCounter::next() . "\n";
echo Counter::describe() . "\n";
echo SeparateCounter::next() . "\n";
echo Counter::describe() . "\n";
Settings::$name .= " runtime";
Settings::$ratio *= 1.5;
echo Settings::$name . ":" . Settings::$ratio . ":" . Settings::$enabled . "\n";
$values = Settings::$values;
$values[0] = 9;
echo Settings::$values[0] . ":" . $values[0] . "\n";
$counter = new Counter();
echo $counter->instance . "\n";
echo Counter::next() . "\n";
