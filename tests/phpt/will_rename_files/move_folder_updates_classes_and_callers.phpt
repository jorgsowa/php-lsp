===description===
Moving a folder updates the namespace of every class under it and their callers,
without importing classes that move along.
===request===
will_rename_files src/A src/B
===disk:composer.json===
{"autoload": {"psr-4": {"App\\": "src/"}}}
===disk:src/A/Bar.php===
<?php

namespace App\A;

class Bar {}
===disk:src/A/Sub/Baz.php===
<?php

namespace App\A\Sub;

use App\A\Bar;

class Baz extends Bar {}
===disk:src/C/User.php===
<?php

namespace App\C;

use App\A\Foo;
use App\A\Sub\Baz;

class User
{
    public function m(Foo $foo, Baz $baz): \App\A\Bar
    {
        return new \App\A\Bar();
    }
}
===file:src/A/Foo.php===
<?php

namespace App\A;

class Foo
{
    /** @var Bar */
    private $bar;

    public function m(Bar $bar): Sub\Baz
    {
        return new Sub\Baz();
    }
}
===expect===
// src/A/Bar.php
2:10-2:15 → "App\\B"

// src/A/Foo.php
2:10-2:15 → "App\\B"

// src/A/Sub/Baz.php
2:10-2:19 → "App\\B\\Sub"
4:4-4:13 → "App\\B\\Bar"

// src/C/User.php
4:4-4:13 → "App\\B\\Foo"
5:4-5:17 → "App\\B\\Sub\\Baz"
9:43-9:53 → "\\App\\B\\Bar"
11:19-11:29 → "\\App\\B\\Bar"
