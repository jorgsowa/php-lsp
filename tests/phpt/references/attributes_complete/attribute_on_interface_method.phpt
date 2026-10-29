===request===
references include_declaration
===file===
<?php
class Required$0Impl {}
//    ^^^^^^^^^^^^ def

interface Repository {
    #[RequiredImpl]
    //^^^^^^^^^^^^ ref
    public function find(int $id): mixed;
}
