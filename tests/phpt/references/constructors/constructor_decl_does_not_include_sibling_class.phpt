===description===
Cursor on `__construct` in the constructor body of a class with a different
name must not bleed into sibling-class constructor references.
===request===
references include_declaration
===file===
<?php
class Lion {
    public function __con$0struct() {}
    //              ^^^^^^^^^^^ def
}
class Tiger {
    public function __construct() {}
}
new Lion();
//  ^^^^ ref
new Tiger();
