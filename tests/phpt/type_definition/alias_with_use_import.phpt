===description===
Aliased types in use imports are resolved via `collect_file_imports` which
tracks `use X as Y` mappings. Jumping to type definition works correctly.
===request===
type_definition
===file:src/Model/Account.php===
<?php
namespace App\Model;
class Account {}
//    ^^^^^^^ type

===file:src/Service.php===
<?php
namespace App\Service;
use App\Model\Account as UserAccount;
function create(UserAccount $acc$0): void {}
