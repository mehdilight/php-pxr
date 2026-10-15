<?php
// Ordinary PHP-style shared values; unreachable cycles are reclaimed natively.
class Node {
    public string $text = "";
    public mixed $next = null;
    public static mixed $keep = null;
}
function discardCycle(): void {
    $a = new Node;
    $b = new Node;
    $a->text = str_repeat("x", 4096);
    $a->next = $b;
    $b->next = $a;
}
$keep = new Node;
$keep->text = "persistent";
$keep->next = $keep;
Node::$keep = $keep;
for ($i = 0; $i < 10000; $i++) { discardCycle(); }
$last = Node::$keep;
if ($last instanceof Node) { echo $last->text, "\n"; }
