===description===
Unqualified type hint inside a namespace resolves to the namespace-prefixed FQCN.
`Logger` in `namespace App\Service` → `App\Service\Logger`.
===request===
type_definition
===file===
<?php
namespace App\Service;
class Logger {}
//    ^^^^^^ type
function write_log(Logger $l$0): void {}
