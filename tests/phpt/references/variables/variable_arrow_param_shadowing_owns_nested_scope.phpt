===description===
A nested arrow function parameter must own its own `$x` references and must
not merge with an outer `$x` when the cursor is inside the arrow body.
===request===
references include_declaration
===file===
<?php
function demo(int $x): int {
    $outer = $x;
    $calc = fn(int $x$0) => $x + 1;
//                 ^^ def
//                        ^^ ref
    return $calc(2) + $outer + $x;
}
