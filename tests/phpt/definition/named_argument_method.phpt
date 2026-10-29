===description===
Definition on a named argument label jumps to the matching parameter of the called instance method.
===request===
definition
===file===
<?php
class Factory {
    public function make(int $radius) {}
//                           ^^^^^^^ def
}
$f = new Factory();
$f->make(rad$0ius: 2);
