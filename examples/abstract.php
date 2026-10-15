<?php
abstract class Shape {
    abstract protected function area(): float;

    public function describe(): string {
        return "area=" . $this->area();
    }
}

abstract class NamedShape extends Shape {
    abstract public function name(): string;
}

class Square extends NamedShape {
    private float $side;

    public function __construct(float $side) {
        $this->side = $side;
    }

    protected function area(): float {
        return $this->side * $this->side;
    }

    public function name(): string {
        return "square";
    }
}

class UnitSquare extends Square {}

class Circle extends Shape {
    protected function area(): float {
        return 3.14;
    }
}

function show(Shape $shape): void {
    echo $shape->describe() . "\n";
    echo ($shape instanceof Shape ? "shape" : "other") . "\n";
}

$square = new Square(4.0);
echo $square->name() . "\n";
show($square);
show(new UnitSquare(1.0));
show(new Circle());
