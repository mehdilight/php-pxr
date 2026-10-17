<?php
trait AsyncIdentity {
    public static function identity() {
        return static async fn(self $value): self => $value;
    }
}
class Prefix {
    use AsyncIdentity;
    private string $text;
    public function __construct(string $text) { $this->text = $text; }
    public function formatter() {
        return async function(string $value): string {
            await sleepAsync(0);
            return $this->text . $value;
        };
    }
}
function delayedIdentity<T>() {
    return async function(T $value): T {
        await sleepAsync(0);
        return $value;
    };
}
async function main(): void {
    $format = (new Prefix("kept:"))->formatter();
    $offset = 10;
    $add = static async fn(int $value): int => $value + $offset;
    $text = spawn $format("value");
    $number = spawn $add(3);
    echo await $text, ":", await $number, "\n";
    $identity = delayedIdentity<string>();
    echo await $identity("generic"), "\n";
    $traitIdentity = Prefix::identity();
    $prefix = await $traitIdentity(new Prefix("trait:"));
    $traitFormat = $prefix->formatter();
    echo await $traitFormat("value"), "\n";
}
