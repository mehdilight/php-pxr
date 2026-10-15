<?php
interface Named {
    public function name(): string;
}

interface Scored {
    public function score(): int;
}

interface Player extends Named, Scored {}
interface Ranked extends Player, Named {}

abstract class BasePlayer implements Ranked {
    public function score(): int { return 10; }
}

class Human extends BasePlayer {
    public function name(): string { return "human"; }
}

class Expert extends Human {
    public function score(): int { return 20; }
}

class Robot implements Player {
    public function name(): string { return "robot"; }
    public function score(): int { return 30; }
}

function describe(Player $player): void {
    echo $player->name() . ":" . $player->score() . "\n";
    echo ($player instanceof Named ? "named" : "other") . "\n";
    echo ($player instanceof Ranked ? "ranked" : "unranked") . "\n";
}

function identity(Named $value): Named { return $value; }

describe(new Human());
describe(new Expert());
describe(new Robot());
echo identity(new Expert())->name() . "\n";
