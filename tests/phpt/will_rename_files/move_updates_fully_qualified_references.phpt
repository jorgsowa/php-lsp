===description===
Moving a class file rewrites qualified references to it, in code and
docblocks, and imports it for docblock-only references from its old namespace.
===request===
will_rename_files src/A/Foo.php src/B/Foo.php
===disk:composer.json===
{"autoload": {"psr-4": {"App\\": "src/"}}}
===disk:src/C/User.php===
<?php

namespace App\C;

class User
{
    /** @var \App\A\Foo */
    private $foo;

    public function make(\App\A\Foo $foo): string
    {
        new \App\A\Foo();
        return \App\A\Foo::class;
    }
}
===disk:src/A/Docs.php===
<?php

namespace App\A;

class Docs
{
    /** @param Foo $foo */
    public function take($foo) {}
}
===disk:global.php===
<?php

$foo = new App\A\Foo();
===file:src/A/Foo.php===
<?php

namespace App\A;

class Foo {}
===expect===
// global.php
2:11-2:20 → "App\\B\\Foo"

// src/A/Docs.php
3:0-3:0 → "use App\\B\\Foo;\n"

// src/A/Foo.php
2:10-2:15 → "App\\B"

// src/C/User.php
6:13-6:23 → "\\App\\B\\Foo"
9:25-9:35 → "\\App\\B\\Foo"
11:12-11:22 → "\\App\\B\\Foo"
12:15-12:25 → "\\App\\B\\Foo"
