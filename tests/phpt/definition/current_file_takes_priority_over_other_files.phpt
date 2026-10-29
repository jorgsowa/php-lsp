===description===
When a symbol is defined in both the current file and another file, the
current file's definition must be returned (current-file-first search order).
===request===
definition
===file:main.php===
<?php
class Foo {}
//    ^^^ def
$f = new Foo$0();

===file:other.php===
<?php
class Foo {}
