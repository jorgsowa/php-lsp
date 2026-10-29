===description===
Go-to-definition on a class name jumps into another opened file.
===request===
definition
===file:src/Greeter.php===
<?php
class Greeter {}
===file:main.php===
<?php
$g = new Gree$0ter();
===expect===
src/Greeter.php:1:6-1:13
