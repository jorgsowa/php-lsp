===description===
Edge case: a class and one of its methods share the same name.
Cursor on the method call — only the method declaration and call should appear;
the class declaration must not be returned as an extra reference.
===allow-invalid-php===
===request===
references include_declaration
===file===
<?php
class get {
    public function get(): void {}
    //              ^^^ def
}
$obj->ge$0t();
//    ^^^ ref
