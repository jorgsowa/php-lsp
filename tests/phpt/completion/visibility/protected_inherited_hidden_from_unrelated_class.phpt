===description===
Protected members inherited by the receiver's class are hidden inside an unrelated class.
===allow-invalid-php===
===request===
completion
===file===
<?php
class A {
    public $a3;
    protected $a2;
    protected function am() {}
}
class B extends A {}
class Y {
    function f(B $b) {
        $b->$0
    }
}
===expect===
Property    $a3
