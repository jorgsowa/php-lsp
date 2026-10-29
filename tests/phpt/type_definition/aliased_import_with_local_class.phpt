===description===
Aliased import is now correctly resolved.
===request===
type_definition
===file:src/Logger.php===
<?php
namespace App;
class Logger {}
//    ^^^^^^ type

===file:src/Service/Logger.php===
<?php
namespace App\Service;
class Logger {}

===file:src/Processor.php===
<?php
namespace App\Service;
use App\Logger as AppLogger;

function log(AppLogger $l$0): void {}  // Explicitly uses alias
