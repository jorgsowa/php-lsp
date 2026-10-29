===request===
definition
===file:a.php===
<?php
class A { const FOO = 1; }
===file:b.php===
<?php
class B { const FOO = 2; }
//              ^^^ def
===file:main.php===
<?php
echo B::F$0OO;
