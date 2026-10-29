===file===
<?php
class C {
    public function run(int $x, int $y): void {}
}
(new C())->run(x: 1, y: 2, x: 99);
//                         ^^^^^ error: run() argument $x overwrites a previous argument
