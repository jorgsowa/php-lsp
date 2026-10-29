===description===
Defining a function in one file clears the undefined-function error in another.
===file:lib.php===
<?php
===file:main.php===
<?php
$x = helper();
//   ^^^^^^^^ error: Function helper() is not defined
===edit:lib.php===
<?php
function helper(): int { return 1; }
===edit:main.php===
<?php
$x = helper();
$y = other();
//   ^^^^^^^ error: Function other() is not defined
