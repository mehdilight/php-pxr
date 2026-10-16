<?php
class Parcel { public int $n; public array<int> $a; public function __construct(int $n) { $this->n=$n; $this->a=[2,3]; } }
async function make(owned<Parcel> $p): owned<Parcel> { $p->n += $p->a[1]; await sleepAsync(0); return move($p); }
async function arrays(array<int> $a): array<int> { $a[0] += 40; return $a; }
async function echoMixed(mixed $x): mixed { await sleepAsync(0); return $x; }
async function main(): void {
    $obj: owned<Parcel> = new Parcel(39);
    $p = await parallelAsync("make", move($obj));
    echo $p->n, ":", $p->a[0], ":";
    $a = [2]; $b = await parallelAsync("arrays", $a); echo $a[0], ":", $b[0], ":";
    $m = await parallelAsync("echoMixed", ["key" => [42]]);
    if (is_array($m)) { $inner = $m["key"]; if (is_array($inner)) {echo $inner[0];} }
    echo "\n";
}
