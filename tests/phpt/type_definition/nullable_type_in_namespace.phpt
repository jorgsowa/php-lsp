===description===
Nullable type `?ClassName` in a namespace resolves the inner class by FQN.
===request===
type_definition
===file:src/Model/Address.php===
<?php
namespace App\Model;
class Address {}

===file:src/Service/Address.php===
<?php
namespace App\Service;
class Address {}
//    ^^^^^^^ type

===file:src/Handler.php===
<?php
namespace App\Service;
function deliver(?Address $addr$0): void {}
