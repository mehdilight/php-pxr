<?php
class Counter { public static int $value = 10; }
async function square(int $n): int { Counter::$value += $n; await sleepAsync(1); return Counter::$value; }
async function talk(owned<Channel> $ch): int {
    $n = intval(await channelReceiveAsync($ch));
    await channelSendAsync($ch, ["answer" => $n * 2]);
    closeChannel($ch); return 7;
}
async function main(): void {
    $a = spawn parallelAsync("square", 2);
    $b = spawn parallelAsync("square", 3);
    $channel = await channelAsync(1);
    $peer = await channelPeerAsync($channel);
    $worker = spawn parallelAsync("talk", move($peer));
    await channelSendAsync($channel, 21);
    $reply = await channelReceiveAsync($channel);
    if (is_array($reply)) { echo $reply["answer"], ":", await $worker, ":", await $a, ":", await $b, ":", Counter::$value, "\n"; }
    closeChannel($channel);
}
