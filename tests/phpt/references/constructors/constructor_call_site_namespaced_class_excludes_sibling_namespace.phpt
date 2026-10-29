===description===
Cursor on `parent::__construct()` inside a namespaced child class must be
scoped to that namespace and must not return instantiation sites for a
same-short-name class in a different namespace (braced-namespace style).
===request===
references include_declaration
===file:a.php===
<?php
namespace Alpha;
class Widget extends Base {
    public function __construct(int $x) {
        parent::__con$0struct($x);
    }
}

===file:b.php===
<?php
namespace Beta;
class Widget {
    public function __construct(string $s) {}
}

===file:c.php===
<?php
$a = new \Alpha\Widget(1);
//       ^^^^^^^^^^^^^ ref
$b = new \Beta\Widget('x');
