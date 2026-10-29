===description===
A trait-aliased method call must not be flagged `UndefinedMethod`.
===file===
<?php
trait BaseInit {
    public function __construct(int $x) {}
}
class Query {
    use BaseInit { __construct as __constructBase; }
    public function __construct() {
        $this->__constructBase(1);
    }
}
