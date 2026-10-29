===request===
definition
===file:a.php===
<?php
class Other { public static $env = 1; }
===file:b.php===
<?php
class Config { public static $env = 2; }
//                            ^^^ def
===file:main.php===
<?php
echo Config::$e$0nv;
