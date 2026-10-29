===description===
Property access inside a clone-with override array must be visible to rename.
===allow-invalid-php===
===request===
rename coordX
===file===
<?php
class Point { public int $x; public int $y; }
//                        ^ rename
$p = new Point();
$q = clone($p, ['x' => 1]);
echo $q->x$0;
//       ^ rename
