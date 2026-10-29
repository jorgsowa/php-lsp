===description===
`parent` keyword in class parameter resolves to the enclosing class, not the parent.
The `parent` keyword now correctly resolves to the actual parent class.
This is resolved by looking up the inheritance chain via ParsedDoc context.
===request===
type_definition
===file===
<?php
class Base {}
//    ^^^^ type
class Child extends Base {
    public function get_parent(parent $p$0): void {}
}
