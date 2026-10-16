<?php
async function server(owned<TcpListener> $listener): void {
    $stream = await tcpAcceptAsync($listener);
    closeSocket($listener);
    $request = await tcpReadAsync($stream, 1024);
    await tcpWriteAsync($stream, "reply:" . $request);
    tcpShutdownWrite($stream); closeSocket($stream);
}
async function connect(): owned<TcpStream> { return await tcpConnectAsync("127.0.0.1", 19876); }
async function main(): void {
    $listener = await tcpListenAsync("127.0.0.1", 19876);
    $server = spawn server(move($listener));
    $result = await taskResultAsync(connect());
    if (resultOk($result)) {
        $stream = await resultValueAsync($result);
        await tcpWriteAsync($stream, "hello");
        echo await tcpReadAsync($stream, 1024), "\n";
        closeSocket($stream);
    } else { echo "error:", resultError($result); }
    await $server;
}
