<?php
trait Label {
    private string $label = 'item';
    public function text(): string { return $this->label; }
}
trait Counter {
    public static int $calls = 0;
    public function countCall(): int { self::$calls += 1; return self::$calls; }
}
trait LabeledCounter { use Label, Counter; }
class Item { use LabeledCounter; }
class Other { use Counter; }
$item = new Item();
echo $item->text() . ':' . $item->countCall() . ':' . $item->countCall() . "\n";
echo (new Other())->countCall() . "\n";
