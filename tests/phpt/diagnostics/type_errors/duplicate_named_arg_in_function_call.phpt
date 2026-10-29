===file===
<?php
function foo(int $a, int $b): void {}
foo(a: 1, b: 2, a: 3);
//              ^^^^ error: foo() argument $a overwrites a previous argument
