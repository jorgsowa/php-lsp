===description===
Rename a property across multiple files should update declaration and all uses.
When renaming from access site ($obj->prop), all occurrences are updated.
===allow-invalid-php===
===request===
rename total
===file:a.php===
<?php
class Foo {
    public int $count;
    //          ^^^^^ rename
}

===file:b.php===
<?php
$foo = new Foo();
echo $foo->coun$0t;
//         ^^^^^ rename
