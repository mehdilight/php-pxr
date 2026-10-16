<?php
// Finite native HTTP server and two clients; no PHP worker or fixture.
async function greet(): void {
    header("Content-Type: text/plain; charset=UTF-8");
    $name = strval($_GET["name"] ?? $_POST["name"] ?? "world");
    await sleepAsync(0);
    echo "Hello, ", $name, "!\n";
}

async function main(): void {
    $server = spawn httpServeAsync("127.0.0.1", 8767, "greet", 2, 4096);
    await sleepAsync(0);
    $get = spawn tcpExchangeAsync("127.0.0.1", 8767,
        "GET /hello?name=Ada HTTP/1.1\r\nHost: localhost\r\n\r\n", 8192);
    $post = spawn tcpExchangeAsync("127.0.0.1", 8767,
        "POST /hello HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: 8\r\n\r\nname=PXR", 8192);
    echo await $get;
    echo await $post;
    await $server;
}
