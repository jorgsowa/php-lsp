===description===
Moving a global-namespace class file under a PSR-4 prefix adds a namespace
declaration and imports the class in global callers.
===request===
will_rename_files lib/User.php src/User.php
===disk:composer.json===
{"autoload": {"psr-4": {"App\\": "src/", "": "lib/"}}}
===disk:lib/Repo.php===
<?php

declare(strict_types=1);

class Repo
{
    public function find(): User
    {
        return new User();
    }
}
===file:lib/User.php===
<?php

declare(strict_types=1);

class User
{
}
===expect===
// lib/Repo.php
3:0-3:0 → "\nuse App\\User;\n"

// lib/User.php
3:0-3:0 → "\nnamespace App;\n"
