<?php
// A native PXR server and two clients on the same cooperative executor.
// No PHP worker or external server fixture is needed.
async function reply(string $request): string {
    await sleepAsync(0);
    return "reply: " . $request;
}

async function main(): void {
    $server = spawn tcpServeAsync("127.0.0.1", 8766, "reply", 2, 4096);
    await sleepAsync(0); // Give the lazy server a turn to open its listener.

    $first = spawn tcpExchangeAsync("127.0.0.1", 8766, "hello", 4096);
    $second = spawn tcpExchangeAsync("127.0.0.1", 8766, "PXR", 4096);
    echo await $first, "\n";
    echo await $second, "\n";
    await $server; // Two accepted requests, then join handlers and close.
}
