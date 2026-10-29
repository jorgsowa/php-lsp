===description===
Goto-definition on a regular (non-promoted) property access must jump to
the property declaration, not just the class declaration.
===request===
definition
===file===
<?php
class Person {
    public string $name = '';
    //             ^^^^ def
}
$p = new Person();
$p->na$0me;
