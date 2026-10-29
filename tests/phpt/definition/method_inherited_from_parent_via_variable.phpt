===description===
Cursor on a method call via a typed variable resolves to the parent class
that declares the method, not the child class that inherits it.
===request===
definition
===file===
<?php
class Base {
    public function render(): void {}
    //              ^^^^^^ def
}
class Child extends Base {}

$c = new Child();
$c->ren$0der();
