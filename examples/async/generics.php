<?php
// Native generic async functions; PHP's interpreter does not parse this syntax.
async function delayed<T>(T $value): T {
    await sleepAsync(0);
    return $value;
}

async function first<T>(array<T> $values, T $fallback): T {
    await sleepAsync(0);
    return $values[0] ?? $fallback;
}

async function main(): void {
    $number = spawn delayed(42);
    $word = spawn delayed<string>("hello");
    echo await $number, ":", await $word, ":", await first([], 7), "\n";
}
