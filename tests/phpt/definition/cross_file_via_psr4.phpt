===request===
definition
===file:src/Greeter.php===
<?php
namespace App;
class Greeter {
    public function hello(): string { return 'hi'; }
    //              ^^^^^ def
}

===file:src/main.php===
<?php
use App\Greeter;
$g = new Greeter();
$g->hel$0lo();
