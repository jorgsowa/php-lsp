===description===
Protected members a class takes from a trait are hidden inside an unrelated class.
===allow-invalid-php===
===request===
completion
===file===
<?php
trait T {
    protected function tm() {}
    public function tp() {}
}
class A { use T; }
class Y {
    function f(A $a) {
        $a->$0
    }
}
===expect===
Method      tp
