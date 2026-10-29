===description===
Goto-definition for an untyped parameter `function foo($x)` must land on
`$x`, not fail or jump to an unrelated position.
===request===
definition
===file===
<?php
function noop($x): mixed {
    //        ^^ def
    return $x$0;
}
