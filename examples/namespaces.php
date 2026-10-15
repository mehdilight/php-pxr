<?php
namespace First {
    interface Named { public function name(): string; }
    class Item implements Named {
        public function name(): string { return "first"; }
    }
    function label(): string { return "First"; }
}

namespace Second {
    use First\Item as Original;
    use First as Library;
    use function First\label as originalLabel;

    class Item extends Original {
        public function name(): string { return "second"; }
    }
    function label(): string { return "Second"; }
    function display(Library\Named $item): void {
        echo originalLabel() . ":" . label() . ":" . $item->name() . "\n";
    }

    display(new Item());
    display(new \First\Item());
    echo \strlen("namespaces") . ":" . strlen("fallback") . "\n";
    echo namespace\label() . "\n";
}

namespace {
    echo \First\label() . "\n";
}
