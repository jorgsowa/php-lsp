===description===
Two classes `User` in different namespaces. Refs on `App\User` must NOT
include the `new Domain\User()` site.
===request===
references include_declaration
===file:src/app.php===
<?php
namespace App;
class Us$0er {}
//    ^^^^ def
$a = new User();
//       ^^^^ ref

===file:src/domain.php===
<?php
namespace Domain;
class User {}
$b = new User();
