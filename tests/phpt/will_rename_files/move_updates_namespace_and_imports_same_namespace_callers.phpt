===description===
Moving a class file to another PSR-4 directory rewrites its namespace line and
imports it in callers that relied on sharing the old namespace.
===request===
will_rename_files src/Model/User.php src/Domain/User.php
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
===disk:src/Model/Qualified.php===
<?php

namespace App\Model;

class Qualified
{
    public function find(): \App\Model\User
    {
        return new \App\Model\User();
    }
}
===disk:src/Service/Greeter.php===
<?php

namespace App\Service;

use App\Model\User;

class Greeter
{
    public function greet(User $user): void {}
}
===file:src/Model/User.php===
<?php

namespace App\Model;

class User
{
}
===expect===
// src/Model/Repo.php
3:0-3:0 → "use App\\Domain\\User;\n"

// src/Model/User.php
2:10-2:19 → "App\\Domain"

// src/Service/Greeter.php
4:4-4:18 → "App\\Domain\\User"
