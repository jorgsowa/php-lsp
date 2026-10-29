===description===
Renaming a class file within its directory keeps the namespace line and adds
no imports to same-namespace callers.
===request===
will_rename_files src/Model/User.php src/Model/Account.php
===disk:composer.json===
{"autoload": {"psr-4": {"App\\": "src/"}}}
===disk:src/Model/Repo.php===
<?php

namespace App\Model;

class Repo
{
    public function find(): User
    {
        return new User();
    }
}
===file:src/Model/User.php===
<?php

namespace App\Model;

class User
{
}
===expect===
// src/Model/Repo.php
6:28-6:32 → "Account"
8:19-8:23 → "Account"

// src/Model/User.php
4:6-4:10 → "Account"
