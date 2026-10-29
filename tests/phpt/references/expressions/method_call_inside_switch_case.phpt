===allow-invalid-php===
===request===
references include_declaration
===file===
<?php
class Proc {
    public function pro$0cess(): void {}
    //              ^^^^^^^ def
}
switch ($x) {
    case 1: $obj->process(); break;
    //            ^^^^^^^ ref
}
