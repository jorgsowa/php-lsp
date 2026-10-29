===description===
Cursor on `$repo` inside the constructor body itself (as a parameter
variable, not a property access) should resolve to the promoted param decl.
===request===
definition
===file===
<?php
class Builder {
    public function __construct(private string $name) {
    //                          ^^^^^^^^^^^^^^^^^^^^ def
        echo $na$0me;
    }
}
