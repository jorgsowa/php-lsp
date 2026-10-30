===description===
Renaming a class file leaves a property or variable named like the class alone.
===request===
will_rename_files src/A/Foo.php src/A/Bar.php
===disk:composer.json===
{"autoload": {"psr-4": {"App\\": "src/"}}}
===disk:src/A/User.php===
<?php

namespace App\A;

class User
{
    /** @var Foo */
    private $foo;

    public function m(Foo $foo): void
    {
        $this->foo = new Foo();
    }
}
===file:src/A/Foo.php===
<?php

namespace App\A;

class Foo {}
===expect===
// src/A/Foo.php
4:6-4:9 → "Bar"

// src/A/User.php
6:13-6:16 → "Bar"
9:22-9:25 → "Bar"
11:25-11:28 → "Bar"
