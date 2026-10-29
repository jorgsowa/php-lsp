===description===
Method refs on C::add must not include the free-function `add()`.
===allow-invalid-php===
===request===
references include_declaration
===file:a.php===
<?php
class C {
    public function a$0dd() {}
    //              ^^^ def
}

===file:b.php===
<?php
function add() {}
add();
$c->add();
//  ^^^ ref
