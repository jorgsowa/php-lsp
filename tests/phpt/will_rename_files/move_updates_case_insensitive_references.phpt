===description===
Moving a class file rewrites references spelled in a different case, since PHP
class names are case-insensitive.
===request===
will_rename_files src/A/Foo.php src/B/Foo.php
===disk:composer.json===
{"autoload": {"psr-4": {"App\\": "src/"}}}
===disk:src/C/User.php===
<?php

namespace App\C;

class User
{
    /** @var \app\a\foo */
    private $foo;

    public function make(\APP\A\FOO $foo): void
    {
        new \app\a\foo();
    }
}
===disk:src/C/Imported.php===
<?php

namespace App\C;

use app\a\foo;

class Imported
{
    public function make(): foo
    {
        return new foo();
    }
}
===disk:src/A/Sibling.php===
<?php

namespace App\A;

class Sibling
{
    public function make(): void
    {
        new foo();
    }
}
===file:src/A/Foo.php===
<?php

namespace App\A;

class Foo {}
===expect===
// src/A/Foo.php
2:10-2:15 → "App\\B"

// src/A/Sibling.php
3:0-3:0 → "\nuse App\\B\\Foo;\n"

// src/C/Imported.php
4:4-4:13 → "App\\B\\Foo"

// src/C/User.php
6:13-6:23 → "\\App\\B\\Foo"
9:25-9:35 → "\\App\\B\\Foo"
11:12-11:22 → "\\App\\B\\Foo"
