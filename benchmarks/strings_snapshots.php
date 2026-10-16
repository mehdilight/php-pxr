<?php
// Keep occasional immutable snapshots while continuing to append.
$text = "";
$snapshots = [];
$i = 0;
while ($i < 50000) {
    $text .= "abcdefgh";
    if ($i % 5000 == 0) {
        $snapshots[] = $text;
    }
    $i++;
}
echo strlen($text), ":", count($snapshots), ":", strlen($snapshots[0]), ":", strlen($snapshots[9]), "\n";
