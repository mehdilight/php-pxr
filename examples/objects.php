<?php

class Point {
    public int $x;
    public int $y;

    public function __construct(int $x, int $y) {
        $this->x = $x;
        $this->y = $y;
    }

    public function move(int $dx, int $dy): Point {
        $this->x += $dx;
        $this->y += $dy;
        return $this;
    }

    public function manhattan(): int {
        return $this->x + $this->y;
    }
}

class BankAccount {
    private string $owner;
    private int $balance;

    public function __construct(string $owner, int $initial) {
        $this->owner = $owner;
        $this->balance = $initial;
    }

    public function deposit(int $amount): BankAccount {
        $this->balance += $amount;
        return $this;
    }

    public function withdraw(int $amount): bool {
        if ($this->balance >= $amount) {
            $this->balance -= $amount;
            return true;
        }
        return false;
    }

    public function getBalance(): int {
        return $this->balance;
    }

    public function statement(): string {
        return sprintf("%s: %d\n", $this->owner, $this->balance);
    }
}

class Counter {
    public int $count = 0;

    public function inc(): int {
        $this->count++;
        return $this->count;
    }
}

// 1. Basic class instantiation, properties, and methods
$p = new Point(10, 20);
printf("Point initial: (%d, %d)\n", $p->x, $p->y);

// 2. Method chaining returning $this
$p->move(5, -3)->move(2, 4);
printf("Point after move: (%d, %d), manhattan=%d\n", $p->x, $p->y, $p->manhattan());

// 3. Encapsulation & private state
$acc = new BankAccount("Alice", 100);
$acc->deposit(50)->deposit(25);
printf("Balance: %d\n", $acc->getBalance());
$ok = $acc->withdraw(30);
printf("Withdrew 30: %s, balance: %d\n", $ok ? "true" : "false", $acc->getBalance());
$too_much = $acc->withdraw(500);
printf("Withdrew 500: %s, balance: %d\n", $too_much ? "true" : "false", $acc->getBalance());
echo $acc->statement();

// 4. Default property value & operator mutations
$c = new Counter();
printf("Counter start: %d\n", $c->count);
$c->inc();
$c->inc();
printf("Counter after 2 inc: %d\n", $c->count);
$c->count += 10;
printf("Counter after += 10: %d\n", $c->count);
