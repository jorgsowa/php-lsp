===description===
Unqualified type names in non-global namespaces are resolved with namespace context.
===request===
type_definition
===file:src/Model/User.php===
<?php
namespace App\Model;
class User {}
//    ^^^^ type

===file:src/Service/UserService.php===
<?php
namespace App\Service;
use App\Model\User;
class UserService {
    public function getUser(User $user$0): void {}
}
