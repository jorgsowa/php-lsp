===description===
A moved class file imports the functions and constants it used unqualified
from its old namespace, but not global ones, its own, or ones already imported.
===request===
will_rename_files src/A/Foo.php src/B/Foo.php
===disk:composer.json===
{"autoload": {"psr-4": {"App\\": "src/"}, "files": ["src/A/functions.php"]}}
===disk:src/A/functions.php===
<?php

namespace App\A;

const LIMIT = 10;
const OTHER = 1;

function helper(): int { return 1; }
function imported(): int { return 2; }
===file:src/A/Foo.php===
<?php

namespace App\A;

use function App\A\imported;

const OWN = 3;

function own(): int { return OWN; }

class Foo
{
    public function m(): int
    {
        $x = $this->helper + self::LIMIT;
        strlen('a');
        return helper() + helper() + imported() + own() + LIMIT + PHP_INT_MAX + OWN;
    }
}
===expect===
// src/A/Foo.php
2:10-2:15 → "App\\B"
5:0-5:0 → "use function App\\A\\helper;\nuse const App\\A\\LIMIT;\n"
