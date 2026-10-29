===description===
Factory method with explicit return type annotation.
===request===
type_definition
===file===
<?php
class User {}
//    ^^^^ type
class UserFactory {
    public static function create(string $name): User {
        return new User();
    }
}
$u = UserFactory::create('Alice');
$u$0->save();
