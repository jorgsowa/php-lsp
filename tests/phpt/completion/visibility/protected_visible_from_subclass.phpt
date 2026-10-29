===description===
Protected members of a parent class are offered on an instance inside a subclass.
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
class B extends A {
    function f(A $a) {
        $a->$0
    }
}
===expect===
Property    $a2
Property    $a3
Method      am
