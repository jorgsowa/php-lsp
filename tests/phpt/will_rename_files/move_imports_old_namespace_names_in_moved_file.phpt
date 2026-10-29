===description===
A moved class file imports the classes it used from its old namespace without
qualification, in code and in docblocks naming a known class.
===request===
will_rename_files src/A/Foo.php src/B/Foo.php
===disk:composer.json===
{"autoload": {"psr-4": {"App\\": "src/"}}}
===disk:src/A/Base.php===
<?php

namespace App\A;

class Base {}
===disk:src/A/Contract.php===
<?php

namespace App\A;

interface Contract {}
===disk:src/A/Helper.php===
<?php

namespace App\A;

class Helper {}
===disk:src/A/Logger.php===
<?php

namespace App\A;

class Logger {}
===file:src/A/Foo.php===
<?php

namespace App\A;

use Countable;

class Foo extends Base implements Contract
{
    /** @var Helper */
    private $helper;

    /** @var array<int, Logger>|null */
    private $loggers;

    /** @return Ghost */
    public function ghost() {}

    public function m(Helper $h): self
    {
        return new Foo();
    }
}
===expect===
// src/A/Foo.php
2:10-2:15 → "App\\B"
5:0-5:0 → "use App\\A\\Base;\nuse App\\A\\Contract;\nuse App\\A\\Helper;\nuse App\\A\\Logger;\n"
