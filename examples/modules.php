<?php
namespace Demo\App;

require_once __DIR__ . "/modules/Greeter.php";
require_once __DIR__ . "/modules/../modules/Greeter.php";

use Demo\Library\Greeter as Welcome;
use Demo\Library\GreeterContract;
use function Demo\Library\decorate as wrap;

function show(GreeterContract $greeter): void {
    echo wrap($greeter->greet()) . "\n";
}

$greeter = new Welcome("PXR");
show($greeter);
echo Welcome::$created . "\n";
echo ($greeter instanceof \Demo\Library\GreeterContract ? "contract" : "other") . "\n";
echo strlen("module") . "\n";
