===description===
When multiple classes share a short name, the exact FQN match is preferred.
===request===
type_definition
===file:src/Model/User.php===
<?php
namespace App\Model;
class User {}

===file:src/Service/User.php===
<?php
namespace App\Service;
class User {}
//    ^^^^ type

===file:src/main.php===
<?php
namespace App\Service;
function create(User $u$0): void {}
