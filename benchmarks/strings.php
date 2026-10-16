<?php
// Repeated string append; final length and ends keep the output small.
$text = "";
$i = 0;
while ($i < 50000) {
    $text .= "abcdefgh";
    $i++;
}
echo strlen($text), ":", substr($text, 0, 8), ":", substr($text, -8), "\n";
