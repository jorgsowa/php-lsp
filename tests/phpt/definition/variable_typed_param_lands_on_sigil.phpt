===description===
Goto-definition on a typed parameter variable must land on `$param`, not on
the type annotation. Previously, `p.span` (which starts at the type) was
used, so clicking `$x` in `Baz $x` would jump to `B` in `Baz`.
===request===
definition
===file===
<?php
class Baz {}
function foo(Baz $x): void {
//               ^^ def
    $y = $x$0;
}
