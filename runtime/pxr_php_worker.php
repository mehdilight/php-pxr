<?php
// The persistent M4 interpreter boundary. The parent owns fd 3's read end;
// ordinary PHP stdout/stderr stay separate from the binary response stream.
namespace PXR\Bridge;

function readBytes(int $length): string {
    $result = '';
    while (strlen($result) < $length) {
        $part = fread(STDIN, $length - strlen($result));
        if ($part === false || $part === '') { throw new \RuntimeException('truncated interop request'); }
        $result .= $part;
    }
    return $result;
}
function readInt(): int { return unpack('P', readBytes(8))[1]; }
function readString(): string {
    $length = readInt();
    if ($length < 0) { throw new \RuntimeException('invalid interop string length'); }
    return readBytes($length);
}
function writeString(string $value): string { return pack('P', strlen($value)) . $value; }
function readValue(array &$objects, array &$arrays, int $depth = 0): mixed {
    if ($depth > 128) { throw new \RuntimeException('interop nesting exceeds 128 levels'); }
    switch (ord(readBytes(1))) {
        case 0: return null;
        case 1: return readInt();
        case 2: return unpack('e', readBytes(8))[1];
        case 3: return ord(readBytes(1)) !== 0;
        case 4: return readString();
        case 5:
            $id = readInt();
            if ($id > 0) {
                if (!isset($arrays[$id])) { throw new \RuntimeException('unknown PHP array handle'); }
                return $arrays[$id];
            }
            $count = readInt();
            if ($count < 0) { throw new \RuntimeException('invalid interop array length'); }
            $value = [];
            for ($i = 0; $i < $count; $i++) {
                $key = ord(readBytes(1)) === 4 ? readString() : readInt();
                $value[$key] = readValue($objects, $arrays, $depth + 1);
            }
            if (!isset($arrays[$id])) { $arrays[$id] = $value; }
            return $arrays[$id];
        case 6:
            $id = readInt();
            if (!isset($objects[$id])) { throw new \RuntimeException('unknown PHP object handle'); }
            return $objects[$id];
        default: throw new \RuntimeException('invalid interop value tag');
    }
}
function writeValue(mixed $value, \SplObjectStorage $ids, array &$objects, array &$arrays, int $depth = 0): string {
    if ($depth > 128) { throw new \RuntimeException('cyclic or excessively nested interop value'); }
    if ($value === null) { return "\0"; }
    if (is_int($value)) { return "\1" . pack('P', $value); }
    if (is_float($value)) { return "\2" . pack('e', $value); }
    if (is_bool($value)) { return "\3" . ($value ? "\1" : "\0"); }
    if (is_string($value)) { return "\4" . writeString($value); }
    if (is_array($value)) {
        $id = count($arrays) + 1;
        $arrays[$id] = $value;
        $result = "\5" . pack('P', $id) . pack('P', count($value));
        foreach ($value as $key => $item) {
            if (\ReflectionReference::fromArrayElement($value, $key) !== null) {
                throw new \RuntimeException('references cannot cross the PHP interop boundary');
            }
            $result .= is_string($key) ? "\4" . writeString($key) : "\1" . pack('P', $key);
            $result .= writeValue($item, $ids, $objects, $arrays, $depth + 1);
        }
        return $result;
    }
    if (is_object($value)) {
        if (!$ids->contains($value)) {
            $id = count($objects) + 1;
            $ids[$value] = $id;
            $objects[$id] = $value;
        }
        return "\6" . pack('P', $ids[$value]) . writeString(get_class($value));
    }
    throw new \RuntimeException('resources cannot cross the PHP interop boundary');
}
function checkArguments(string $name, array $args): void {
    $function = new \ReflectionFunction($name);
    foreach ($function->getParameters() as $index => $parameter) {
        if ($parameter->isPassedByReference() && array_key_exists($index, $args)) {
            throw new \RuntimeException('by-reference arguments cannot cross the PHP interop boundary');
        }
    }
}

$__pxr_out = fopen('php://fd/3', 'wb');
$__pxr_ids = new \SplObjectStorage();
$__pxr_objects = [];
$__pxr_arrays = [];
while (($__pxr_op = fread(STDIN, 1)) !== false && $__pxr_op !== '') {
    try {
        if ($__pxr_op === 'I') {
            $__pxr_mode = ord(readBytes(1));
            $__pxr_path = readString();
            // Execute includes in the worker's global scope, preserving their
            // globals and PHP's own include_once/require_once registry.
            $__pxr_result = match ($__pxr_mode) {
                0 => include $__pxr_path,
                1 => include_once $__pxr_path,
                2 => require $__pxr_path,
                3 => require_once $__pxr_path,
                default => throw new \RuntimeException('invalid include mode'),
            };
            // Include statements discard their value, including arbitrary objects.
            $__pxr_result = null;
        } elseif ($__pxr_op === 'C') {
            $__pxr_name = readString();
            $__pxr_fallback = readString();
            if (!function_exists($__pxr_name) && $__pxr_fallback !== '') { $__pxr_name = $__pxr_fallback; }
            $__pxr_args = readValue($__pxr_objects, $__pxr_arrays);
            checkArguments($__pxr_name, $__pxr_args);
            $__pxr_result = $__pxr_name(...$__pxr_args);
        } elseif ($__pxr_op === 'S') {
            $__pxr_result = strval(readValue($__pxr_objects, $__pxr_arrays));
        } elseif ($__pxr_op === 'B') {
            $__pxr_operator = ord(readBytes(1));
            $__pxr_left = readValue($__pxr_objects, $__pxr_arrays);
            $__pxr_right = readValue($__pxr_objects, $__pxr_arrays);
            $__pxr_result = match ($__pxr_operator) {
                10 => $__pxr_left == $__pxr_right,
                11 => $__pxr_left != $__pxr_right,
                12 => $__pxr_left < $__pxr_right,
                13 => $__pxr_left <= $__pxr_right,
                14 => $__pxr_left > $__pxr_right,
                15 => $__pxr_left >= $__pxr_right,
                16 => $__pxr_left <=> $__pxr_right,
                17 => $__pxr_left === $__pxr_right,
                default => throw new \RuntimeException('invalid comparison operation'),
            };
        } else {
            throw new \RuntimeException('invalid interop operation');
        }
        $__pxr_response = "\0" . writeValue($__pxr_result, $__pxr_ids, $__pxr_objects, $__pxr_arrays);
    } catch (\Throwable $__pxr_error) {
        $__pxr_response = "\1" . writeString(get_class($__pxr_error) . ': ' . $__pxr_error->getMessage());
    }
    // Complete all PHP output before acknowledging this operation.
    fflush(STDOUT);
    for ($__pxr_offset = 0; $__pxr_offset < strlen($__pxr_response);) {
        $__pxr_written = fwrite($__pxr_out, substr($__pxr_response, $__pxr_offset));
        if ($__pxr_written === false || $__pxr_written === 0) { exit(255); }
        $__pxr_offset += $__pxr_written;
    }
    fflush($__pxr_out);
}
