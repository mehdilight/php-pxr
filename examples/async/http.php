<?php
// Run: pxr run examples/async/http.php
// Then: curl 'http://127.0.0.1:8080/hello?name=Ada'
async function handleHttp(): void {
    header("Content-Type: text/plain; charset=UTF-8");
    if ($_SERVER["PATH_INFO"] === "/health") {
        echo "ok\n";
        return;
    }
    if ($_SERVER["PATH_INFO"] === "/hello") {
        $name = strval($_GET["name"] ?? $_POST["name"] ?? "world");
        await sleepAsync(0);
        echo "Hello, ", $name, "!\n";
        return;
    }
    http_response_code(404);
    echo "Not found\n";
}

async function main(): void {
    await httpServeAsync("127.0.0.1", 8080, "handleHttp", 0, 1048576);
}
