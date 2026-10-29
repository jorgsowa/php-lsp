===description===
A closure with its own local variable must be treated as a nested
variable owner instead of being merged into the enclosing function scope.
===request===
references include_declaration
===file===
<?php
function demo(int $x): int {
    $outer = $x;
    $calc = function () use ($outer): int {
        $x = $outer + 1;
//      ^^ def
        return $x$0;
//             ^^ ref
    };
    return $calc(2) + $x;
}
