===description===
Renaming from a property declaration site should update the declaration and
all access sites, just like renaming from an access site.
===allow-invalid-php===
===request===
rename total
===file===
<?php
class Foo {
    public int $coun$0t;
    //          ^^^^^ rename
}
$foo = new Foo();
$foo->count++;
//    ^^^^^ rename
echo $foo->count;
//         ^^^^^ rename
