===description===
Unqualified type hints resolve within the same namespace.
===request===
type_definition
===file:src/Logger.php===
<?php
namespace App;
class Logger {}
//    ^^^^^^ type

===file:src/Service.php===
<?php
namespace App;
class Service {
    public function log(Logger $l$0): void {}
}
