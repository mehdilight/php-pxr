<?php
function greet(string $name = "world"): string { return "hello " . $name; }
$greet = greet(...);
echo $greet(), "\n";
echo $greet(name: "native"), "\n";
$length = strlen(...);
echo $length("native"), "\n";
class ReferenceExample {
    public string $prefix = "bound:";
    public function text(string $value): string { return $this->prefix . $value; }
}
$object = new ReferenceExample();
$text = $object->text(...);
$object->prefix = "updated:";
echo $text("native"), "\n";
