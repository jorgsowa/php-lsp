===allow-invalid-php===
===request===
rename $col
===file:a.php===
<?php
class Point {
    public function __construct(int $x$0) { echo $x; }
    //                              ^^ rename
    //                                         ^^ rename
}

===file:b.php===
<?php
$p = new Point(x: 1);
//             ^ rename
