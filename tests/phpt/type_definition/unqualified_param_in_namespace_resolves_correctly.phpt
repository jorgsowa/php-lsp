===description===
Unqualified type names in non-global namespaces are resolved with namespace context.
`Logger $l` in `namespace App\Service` resolves to `App\Service\Logger` via resolve_fqn.
===request===
type_definition
===file:src/Logger.php===
<?php
namespace App\Service;
class Logger {}
//    ^^^^^^ type

===file:src/Service.php===
<?php
namespace App\Service;
class Service {
    public function log(Logger $l$0): void {}
}
