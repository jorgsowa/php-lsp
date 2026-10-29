===file===
<?php
class Point {
    public function __construct(public int $x, public int $y) {}
}
new Point(x: 0, y: 1, x: 2);
//                    ^^^^ error: Point::__construct() argument $x overwrites a previous argument
