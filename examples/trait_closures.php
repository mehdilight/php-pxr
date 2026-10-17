<?php
class BaseValue {
    public string $text;
    public function __construct(string $text) { $this->text = $text; }
}
trait ClosureFactory {
    public static function formatter() {
        return static fn(string $value): string => self::$prefix . $value;
    }
    public function identity() { return fn(self $value): self => $value; }
    public static function ancestor() { return static fn(parent $value): parent => $value; }
}
class FirstValue extends BaseValue {
    use ClosureFactory;
    private static string $prefix = "first:";
}
class SecondValue extends BaseValue {
    use ClosureFactory;
    private static string $prefix = "second:";
}
$first = FirstValue::formatter();
$second = SecondValue::formatter();
echo $first("value"), ":", $second("value"), "\n";
$identity = (new FirstValue("creator"))->identity();
$ancestor = SecondValue::ancestor();
echo $identity(new FirstValue("kept"))->text, ":", $ancestor(new BaseValue("parent"))->text, "\n";
