<?php
// M10's delivered syntax: shared assignments still preserve PHP value semantics.
$scores = ["first" => [1, 2], "second" => [3]];
$snapshot = $scores;
$first = ++$scores["first"][0];
$second = ($scores["second"][0] += 4);
echo "updated: $first, $second; snapshot: {$snapshot['first'][0]}\n";

foreach ($scores as $name => $row) {
    echo "$name: {$row[0]}\n";
}

class Report {
    public mixed $rows = null;
}
$report = new Report();
$report->rows["summary"]["total"] = $first + $second;
echo "total: {$report->rows['summary']['total']}\n";

$lookup = ["existing" => 4];
$value = ($lookup["fallback"] ??= 9);
echo "fallback: $value; existing: {$lookup['existing']}\n";
