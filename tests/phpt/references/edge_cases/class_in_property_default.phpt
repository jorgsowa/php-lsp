===allow-invalid-php===
===request===
references include_declaration
===file===
<?php
class Sta$0tus {
//    ^^^^^^ def
    const ACTIVE = 1;
}
class Foo {
    public int $state = Status::ACTIVE;
    //                  ^^^^^^ ref
}
