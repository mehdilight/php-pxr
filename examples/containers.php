<?php
function containerValue(mixed $value): mixed { return $value; }
function describeContainer(mixed $value): string {
    if (is_array($value)) {
        $text = 'array:' . count($value) . ':';
        foreach ($value as $item) { $text .= strval($item) . '|'; }
        return $text;
    }
    if ($value instanceof NamedItem) { return $value->label(); }
    return strval($value);
}
class NamedItem {
    public string $name = 'item';
    public function label(): string { return 'object:' . $this->name; }
    public function __toString(): string { return $this->label(); }
}
$array = containerValue([1, 'two', false, null]);
$copy = $array;
if (is_array($copy)) { $copy[0] = 'changed'; }
echo describeContainer($array) . "\n";
echo describeContainer($copy) . "\n";
$object = new NamedItem();
$alias = containerValue($object);
if ($alias instanceof NamedItem) { $alias->name = 'shared'; }
echo describeContainer($alias) . ':' . $object->name . ':' . $alias . "\n";
echo (containerValue('6') + 2) . ':' . (containerValue('01') == 1) . "\n";
