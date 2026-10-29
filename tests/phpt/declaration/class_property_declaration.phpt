===request===
declaration
===file===
<?php
class User {
    public string $name$0;
    //             ^^^^ decl
    public function getName(): string { return $this->name; }
}
