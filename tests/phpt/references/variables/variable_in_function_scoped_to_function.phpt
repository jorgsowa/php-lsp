===description===
References for a function parameter variable must be scoped to that
function and not include the same-named variable in a sibling function.
===request===
references include_declaration
===file===
<?php
function foo(int $x$0): int {
    //           ^^ def
    $y = $x + 1;
    //   ^^ ref
    return $x;
    //     ^^ ref
}
function bar(int $x): int { return $x; }
