===description===
Same as above but the namespace is declared via the simple (no-brace) style.
===request===
references include_declaration
===file:a.php===
<?php
namespace Alpha;
class Box extends Base {
    public function __construct(int $n) {
        parent::__con$0struct($n);
    }
}

===file:b.php===
<?php
namespace Beta;
class Box {
    public function __construct(string $s) {}
}

===file:c.php===
<?php
$a = new \Alpha\Box(1);
//       ^^^^^^^^^^ ref
$b = new \Beta\Box('x');
