===description===
Deeply nested namespace (A\B\C) — resolve_fqn must handle multi-segment prefix.
===request===
type_definition
===file:src/Cmd.php===
<?php
namespace App\Console\Command;
class Cmd {}
//    ^^^ type

===file:src/Other/Cmd.php===
<?php
namespace App\Http\Controller;
class Cmd {}

===file:src/Dispatch.php===
<?php
namespace App\Console\Command;
function dispatch(Cmd $c$0): void {}
