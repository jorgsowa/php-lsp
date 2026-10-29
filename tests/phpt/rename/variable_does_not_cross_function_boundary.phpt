===description===
Rename a variable should only affect the same scope, not variables with the
same name in other function scopes.
===allow-invalid-php===
===request===
rename $y
===file===
<?php
function foo() { $x$0 = 1; }
//               ^^ rename
function bar() { $x = 2; }
