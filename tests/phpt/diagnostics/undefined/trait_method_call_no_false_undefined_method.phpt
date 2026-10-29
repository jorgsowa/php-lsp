===description===
Control: a plain (un-aliased) trait method call produces no diagnostics.
===file===
<?php
trait BaseInit {
    public function init(int $x): void {}
}
class Query {
    use BaseInit;
    public function __construct() {
        $this->init(1);
    }
}
