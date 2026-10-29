===request===
references include_declaration
===file:src/Greeter.php===
<?php
namespace App;
class Greeter {
    public function hel$0lo(): string { return 'hi'; }
    //              ^^^^^ def
}

===file:src/main.php===
<?php
use App\Greeter;
$g = new Greeter();
$g->hello();
//  ^^^^^ ref
