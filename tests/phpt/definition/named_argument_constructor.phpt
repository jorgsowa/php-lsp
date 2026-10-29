===description===
Definition on a named argument of `new` jumps to the constructor parameter, including a promoted one.
===request===
definition
===file===
<?php
class Circle {
    public function __construct(public float $radius) {}
//                                           ^^^^^^^ def
}
new Circle(rad$0ius: 2.0);
