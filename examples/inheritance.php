<?php

class Animal {
    public string $name;
    protected int $energy;

    public function __construct(string $name, int $energy) {
        $this->name = $name;
        $this->energy = $energy;
    }

    public function speak(): string {
        return $this->name . " makes a generic sound";
    }

    public function getEnergy(): int {
        return $this->energy;
    }
}

class Dog extends Animal {
    public string $breed;

    public function __construct(string $name, int $energy, string $breed) {
        parent::__construct($name, $energy);
        $this->breed = $breed;
    }

    public function speak(): string {
        $parentSound = parent::speak();
        return $parentSound . ", but specifically " . $this->name . " barks! (energy: " . $this->energy . ", breed: " . $this->breed . ")";
    }
}

class Puppy extends Dog {
    public function speak(): string {
        return $this->name . " yips softly!";
    }
}

function makeAnimalSpeak(Animal $a): void {
    echo $a->speak() . "\n";
}

$a = new Animal("Generic", 10);
$d = new Dog("Buddy", 20, "Golden");
$p = new Puppy("Tiny", 5, "Chihuahua");

makeAnimalSpeak($a);
makeAnimalSpeak($d);
makeAnimalSpeak($p);

echo "instanceof checks:\n";
echo ($a instanceof Animal ? "yes" : "no") . "\n";
echo ($a instanceof Dog ? "yes" : "no") . "\n";
echo ($d instanceof Animal ? "yes" : "no") . "\n";
echo ($d instanceof Dog ? "yes" : "no") . "\n";
echo ($p instanceof Animal ? "yes" : "no") . "\n";
echo ($p instanceof Dog ? "yes" : "no") . "\n";
echo ($p instanceof Puppy ? "yes" : "no") . "\n";
