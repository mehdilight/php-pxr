<?php
class X { public int $value; public function __construct(int $v) { $this->value=$v; } }
async function fail(): int { await sleepAsync(-1); return 1; }
async function make(): owned<X> { await sleepAsync(0); return new X(42); }
async function main(): void {
    $failed = spawn taskResultAsync(fail());
    $made = spawn taskResultAsync(make());
    await sleepAsync(0);
    $a = await $failed; echo resultOk($a), resultStatus($a), ":", resultError($a) > 0, ":";
    drop($a);
    $b = await $made; echo resultOk($b), ":";
    $obj = await resultValueAsync($b); echo $obj->value, "\n";
}
