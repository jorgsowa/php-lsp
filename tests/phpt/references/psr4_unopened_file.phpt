===description===
References find uses in a PSR-4 file that is on disk but never opened.
===request===
references
===disk:composer.json===
{"autoload": {"psr-4": {"App\\": "src/"}}}
===disk:src/User.php===
<?php
namespace App;
function use_greeter(): void { (new Greeter())->hello(); }
===file:src/Greeter.php===
<?php
namespace App;
class Greeter {
    public function hel$0lo(): void {}
}
===expect===
src/User.php:2:48-2:53
