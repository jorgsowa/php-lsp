===description===
Protected members are hidden inside a class unrelated to the receiver's class.
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
class Y {
    function f(A $a) {
        $a->$0
    }
}
===expect===
Property    $a3
