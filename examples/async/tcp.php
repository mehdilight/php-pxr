<?php
// Start tcp_server.py first; this PXR extension runs without PHP.
async function main(): void {
    $unused = tcpExchangeAsync("127.0.0.1", 8765, "unused", 4096);
    cancelTask($unused); // Lazy cancellation opens no connection.

    $first = spawn tcpExchangeAsync("127.0.0.1", 8765, "hello", 4096, 5000);
    $second = spawn tcpExchangeAsync("127.0.0.1", 8765, "PXR", 4096, 5000);
    echo await $first, "\n", await $second, "\n";
}
