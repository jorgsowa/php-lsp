===description===
Method defined on a base class must also find calls on subclass receivers.
===request===
references include_declaration
===file===
<?php
class Base {
    public function wo$0rk(): void {}
    //              ^^^^ def
}
class Child extends Base {}
$c = new Child();
$c->work();
//  ^^^^ ref
