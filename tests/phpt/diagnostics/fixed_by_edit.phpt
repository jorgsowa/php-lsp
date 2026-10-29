===description===
An undefined-function error disappears once the other file defines it.
===request===
diagnostics
===file:lib.php===
<?php
===file:main.php===
<?php
helper();
===expect===
lib.php: <empty>
main.php: 1:0-1:8 [1] UndefinedFunction: Function helper() is not defined
===edit:lib.php===
<?php
function helper(): void {}
===expect===
lib.php: <empty>
main.php: <empty>
