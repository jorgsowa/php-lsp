===request===
references include_declaration
===file:main.php===
<?php
use const App\LI$0MIT;
//        ^^^^^^^^^ ref

===file:App/constants.php===
<?php
namespace App;
const LIMIT = 100;
//    ^^^^^ def
