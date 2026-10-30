===description===
Renaming a class file within its directory renames the class in docblocks,
unqualified, qualified and imported, but keeps aliases.
===request===
will_rename_files src/Model/User.php src/Model/Account.php
===disk:composer.json===
{"autoload": {"psr-4": {"App\\": "src/"}}}
===disk:src/Model/Repo.php===
<?php

namespace App\Model;

class Repo
{
    /** @var User */
    private $user;

    /** @param array<int, User>|null $users */
    public function all($users) {}
}
===disk:src/Service/Svc.php===
<?php

namespace App\Service;

use App\Model\User;
use App\Model\User as U;

class Svc
{
    /** @var User */
    private $a;

    /** @var U */
    private $b;

    /** @var \App\Model\User */
    private $c;
}
===file:src/Model/User.php===
<?php

namespace App\Model;

class User
{
    /** @return User */
    public function self() { return $this; }
}
===expect===
// src/Model/Repo.php
6:13-6:17 → "Account"
9:26-9:30 → "Account"

// src/Model/User.php
4:6-4:10 → "Account"
6:16-6:20 → "Account"

// src/Service/Svc.php
4:4-4:18 → "App\\Model\\Account"
5:4-5:18 → "App\\Model\\Account"
9:13-9:17 → "Account"
15:13-15:28 → "\\App\\Model\\Account"
