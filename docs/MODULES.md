# Namespaces and compile-time modules

PXR compiles an entry file and its declaration-only dependencies into one native
program. There is no runtime PHP loader or interpreter dependency.

A library file, `Library.php`:

```php
<?php
namespace Example\Library;

class Greeting {
    public static function text(): string {
        return "Hello from a module";
    }
}

function suffix(): string { return "!"; }
```

An entry file in the same directory:

```php
<?php
namespace Example\App;

require_once __DIR__ . "/Library.php";

use Example\Library\Greeting as Message;
use function Example\Library\suffix;

echo Message::text() . suffix() . "\n";
```

Compile with `pxr build main.php -o app` or run with `pxr run main.php`. The same
example can be run under PHP. The repository's `examples/modules.php` is tested
against PHP and demonstrates interface types and static storage across files.

## Name resolution

Use either `namespace Example\App;` for a namespace extending to the next
namespace declaration/end of file, or `namespace Example\App { ... }` for an
explicit block. Braced files can also contain `namespace { ... }` global blocks.
Do not mix the two styles or place code outside the blocks in a braced file.

Class/interface names resolve through the current namespace and class/namespace
imports. `use Example\Library as Lib;` permits `Lib\Greeting`. A leading `\`
selects an absolute name; `namespace\Thing` selects the current namespace without
consulting aliases. Class references never fall back to the global namespace.

Unqualified function calls first look for a function in the current namespace,
then a global function or builtin. `use function` binds a call to the imported
function without fallback. Qualified calls also have no fallback. Aliases are
case-insensitive and local to their file and namespace block; imported names do
not autoload files. Use `require_once` to provide those declarations.

## Dependency rules

* `require` and `require_once` are supported as standalone top-level statements
  with a literal UTF-8 string path, optionally parenthesized. The literal can be
  prefixed with `__DIR__ .`.
* Relative literals resolve from the containing file's directory. Absolute paths
  remain absolute. `__DIR__ . "/file.php"` uses the containing file's directory
  explicitly and avoids differences from PHP's working-directory/include-path
  search behavior.
* Dependencies must appear before executable top-level statements. Loaded files
  may contain class/interface/function declarations, namespace/import directives,
  empty statements, and further dependencies. Runtime side effects, top-level
  variable initialization, and return statements in dependencies are rejected.
* `require_once` deduplicates canonical paths, including equivalent paths through
  `..` or symlinks. Shared dependency cycles are safe when the revisited edge uses
  `require_once`; non-once cycles are compile errors. Repeated plain `require` of
  the same declarations produces duplicate-definition diagnostics.
* Missing/unreadable files, NUL-containing paths, dynamic paths, conditional or
  function-local dependencies, and excessive dependency depth are errors.
* `check`, `build`, `run`, `emit-ir`, and `emit-llvm` resolve the same graph.
  Dependency diagnostics identify the source file and include its source excerpt.
  `tokens` and `ast` inspect only the requested file.

## Remaining limits

Grouped imports, constant imports, autoloading, dynamic class/function names,
`include`/`include_once`, include expressions/return values, PHP `include_path`,
and runtime inclusion of untyped PHP are not implemented. This module feature
provides compile-time declarations; runtime PHP interop remains in M4.
