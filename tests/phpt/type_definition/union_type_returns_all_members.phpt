===description===
Union types (PHP 8.0+) return all matching types in the union.
===request===
type_definition
===file===
<?php
class Admin {}
//    ^^^^^ type
class User {}
//    ^^^^ type
function authenticate(Admin|User $a$0): void {}
