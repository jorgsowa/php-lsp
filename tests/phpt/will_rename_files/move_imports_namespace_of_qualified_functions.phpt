===description===
A moved class file imports the sub-namespace its qualified function calls and
constants resolved through, once, and not for unknown names.
===request===
will_rename_files src/A/Foo.php src/B/Foo.php
===disk:composer.json===
{"autoload": {"psr-4": {"App\\": "src/"}, "files": ["src/A/Util/functions.php", "src/A/Fmt/functions.php"]}}
===disk:src/A/Util/functions.php===
<?php

namespace App\A\Util;

const LIMIT = 10;

function helper(): int { return 1; }
===disk:src/A/Fmt/functions.php===
<?php

namespace App\A\Fmt;

const WIDTH = 80;
===file:src/A/Foo.php===
<?php

namespace App\A;

class Foo
{
    public function m(): int
    {
        Missing\run();
        return Util\helper() + Util\LIMIT + Fmt\WIDTH;
    }
}
===expect===
// src/A/Foo.php
2:10-2:15 → "App\\B"
3:0-3:0 → "\nuse App\\A\\Fmt;\nuse App\\A\\Util;\n"
