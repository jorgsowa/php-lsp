===request===
references include_declaration
===file===
<?php
class Permiss$0ion {}
//    ^^^^^^^^^^ def

class AdminResource {
    #[Permission('read')]
    //^^^^^^^^^^ ref
    #[Permission('admin')]
    //^^^^^^^^^^ ref
    public function manage(): void {}
}
