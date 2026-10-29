===request===
declaration
===file:helpers.php===
<?php
function validate(string $x): bool { return true; }
//       ^^^^^^^^ decl

===file:main.php===
<?php
$ok = validate$0('test');
