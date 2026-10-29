===description===
Variable goto-definition jumps to the first assignment in scope.
===request===
definition
===file===
<?php
function foo() {
    $x = 1;
//  ^^ def
    return $x$0;
}
