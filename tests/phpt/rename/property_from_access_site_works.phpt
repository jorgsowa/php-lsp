===description===
Rename from an access site also updates the declaration and all other accesses.
===allow-invalid-php===
===request===
rename total
===file===
<?php
class Foo {
    public int $count;
    //          ^^^^^ rename
}
$foo = new Foo();
$foo->coun$0t++;
//    ^^^^^ rename
echo $foo->count;
//         ^^^^^ rename
