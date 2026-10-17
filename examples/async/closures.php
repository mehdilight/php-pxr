<?php
class Prefix {
    private string $text;
    public function __construct(string $text) { $this->text = $text; }
    public function formatter() {
        return async function(string $value): string {
            await sleepAsync(0);
            return $this->text . $value;
        };
    }
}
async function main(): void {
    $format = (new Prefix("kept:"))->formatter();
    $offset = 10;
    $add = static async fn(int $value): int => $value + $offset;
    $text = spawn $format("value");
    $number = spawn $add(3);
    echo await $text, ":", await $number, "\n";
}
