===description===
A global-namespace function called unqualified from inside a different
namespace, with no `use function` import, must not be flagged
`UndefinedFunction` — PHP falls back to the global function at runtime.
===file===
<?php
namespace {
    function global_only_helper(int $x): int { return $x * 2; }
}

namespace App {
    function run(int $x): int {
        return global_only_helper($x);
    }
}
