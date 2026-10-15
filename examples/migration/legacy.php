<?php
// @pxr-dynamic
// This file stays in PHP: untyped parameters, default arguments, exceptions,
// global state, and library calls need no changes for the compiled entry point.
$legacyCalls = 0;
function legacyCatalog($category = 'fruit') {
    global $legacyCalls;
    $legacyCalls++;
    $stream = fopen(__DIR__ . '/catalog.csv', 'rb');
    if ($stream === false) { throw new RuntimeException('cannot open catalog'); }
    fgetcsv($stream, 0, ',', '"', ''); // header
    $items = [];
    while (($row = fgetcsv($stream, 0, ',', '"', '')) !== false) {
        if ($row[0] === $category) {
            $items[] = ['name' => $row[1], 'price' => (int)$row[2]];
        }
    }
    fclose($stream);
    return ['category' => strtoupper($category), 'items' => $items, 'calls' => $legacyCalls];
}
function legacyPrice($prices) { return array_sum($prices); }
function legacyRoundtrip($value) { return $value; }
function legacyObject() { static $object; return $object ??= (object)['count' => 0]; }
function legacyIncrement($object) { return ++$object->count; }
