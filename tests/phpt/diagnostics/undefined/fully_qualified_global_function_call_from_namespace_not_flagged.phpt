===description===
A leading-backslash reference to a global function from inside a
namespace is the explicit, unambiguous form of the fallback above and
must not be flagged either.
===file===
<?php
namespace {
    function global_only_helper(int $x): int { return $x * 2; }
}

namespace App {
    function run(int $x): int {
        return \global_only_helper($x);
    }
}
