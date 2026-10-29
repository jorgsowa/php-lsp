===description===
A plain `/* ... */` block comment (not a doc-block: no second `*`) must
not be treated as PHPDoc at all — `@word`-shaped text inside one still
goes through ordinary bareword resolution.
===allow-invalid-php===
===request===
references include_declaration
===file===
<?php
function param(): void {}
//       ^^^^^ def
/* @param int $x */
$x = par$0am();
//   ^^^^^ ref
