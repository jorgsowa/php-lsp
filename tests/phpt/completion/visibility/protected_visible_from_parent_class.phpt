===description===
Protected members declared in a subclass are offered inside its parent class.
===allow-invalid-php===
===request===
completion
===file===
<?php
class A {
    function f(B $b) {
        $b->$0
    }
}
class B extends A {
    public $b1;
    protected $b2;
}
===expect===
Property    $b1
Property    $b2
Method      f
