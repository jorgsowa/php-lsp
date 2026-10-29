===allow-invalid-php===
===request===
references include_declaration
===file===
<?php
class Obj {
    public function getP$0rop(): mixed { return null; }
    //              ^^^^^^^ def
}
unset($obj->getProp());
//          ^^^^^^^ ref
