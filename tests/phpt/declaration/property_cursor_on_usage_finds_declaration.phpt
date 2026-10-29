===request===
declaration
===file===
<?php
class User {
    public string $email;
    //             ^^^^^ decl
}
$u = new User();
$u->email$0 = 'test@example.com';
