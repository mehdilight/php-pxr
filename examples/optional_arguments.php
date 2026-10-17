<?php
function greet(string $name = "world", string $suffix = "!"): string {
    return "hello " . $name . $suffix;
}
interface Message {
    public function value(string $text = "interface"): string;
}
class BaseMessage implements Message {
    public function value(string $text = "base"): string { return $text; }
}
class ChildMessage extends BaseMessage {
    public function value(string $text = "child"): string { return $text; }
}
function message(Message $value): string { return $value->value(); }

$callback = fn(int $number = 7): int => $number;
echo greet(), "\n", greet("native", "."), "\n";
echo message(new ChildMessage()), ":", $callback(), "\n";
