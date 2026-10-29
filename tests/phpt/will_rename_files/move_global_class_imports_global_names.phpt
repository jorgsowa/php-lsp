===description===
A global-namespace class file moved under a PSR-4 prefix imports the global
classes it used, below its new namespace declaration.
===request===
will_rename_files lib/User.php src/User.php
===disk:composer.json===
{"autoload": {"psr-4": {"App\\": "src/", "": "lib/"}}}
===disk:lib/Base.php===
<?php

class Base {}
===file:lib/User.php===
<?php

declare(strict_types=1);

class User extends Base
{
    public function fail(): never
    {
        throw new \LogicException(Exception::class);
    }
}
===expect===
// lib/User.php
3:0-3:0 → "\nnamespace App;\n\nuse Base;\nuse Exception;\n"
