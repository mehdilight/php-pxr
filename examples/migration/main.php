<?php
require_once __DIR__ . '/legacy.php';
require_once __DIR__ . '/legacy.php';

function renderCatalog(mixed $catalog): string {
    if (!is_array($catalog)) { return 'invalid catalog'; }
    $label = $catalog['category'];
    $items = $catalog['items'];
    if (!is_string($label) || !is_array($items)) { return 'invalid catalog'; }
    $total = 0;
    foreach ($items as $item) {
        if (!is_array($item)) { return 'invalid item'; }
        $price = $item['price'];
        if (!is_int($price)) { return 'invalid price'; }
        $total += $price;
    }
    return $label . ':' . count($items) . ':' . $total . ':' . $catalog['calls'];
}
echo renderCatalog(legacyCatalog('fruit')) . "\n";
echo renderCatalog(legacyCatalog('tools')) . "\n";
echo legacyPrice([3, 5, 7]) . "\n";
$object = legacyObject();
echo is_object($object) . ':' . ($object === legacyObject()) . ':' . legacyIncrement($object) . ':' . legacyIncrement($object) . "\n";
$record = legacyRoundtrip(['name' => 'native', 'count' => 2]);
echo renderCatalog(legacyCatalog()) . "\n";
