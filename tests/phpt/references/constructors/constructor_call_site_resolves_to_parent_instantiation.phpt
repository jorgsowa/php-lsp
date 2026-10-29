===description===
Cursor on `parent::__construct()` resolves to the parent class's own
instantiation sites, even when the child class itself is never
instantiated directly.
===request===
references include_declaration
===file===
<?php
class Alpha {
    public function __construct() {}
}
class Beta extends Alpha {
    public function __construct() {
        parent::__con$0struct();
        //      ^^^^^^^^^^^ ref
    }
}
new Alpha();
//  ^^^^^ ref
