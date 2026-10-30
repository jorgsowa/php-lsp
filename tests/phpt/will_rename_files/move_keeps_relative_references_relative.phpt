===description===
Moving a class file keeps namespace-relative references relative when the new
name still resolves from there, and fully qualifies them when an import shadows it.
===request===
will_rename_files src/A/Foo.php src/B/Foo.php
===disk:composer.json===
{"autoload": {"psr-4": {"App\\": "src/"}}}
===disk:src/Relative.php===
<?php

namespace App;

class Relative
{
    /** @var A\Foo */
    private $foo;

    public function make(): A\Foo
    {
        return new namespace\A\Foo();
    }
}
===disk:src/Shadowed.php===
<?php

namespace App;

use Other\B;

class Shadowed
{
    public function make(): A\Foo
    {
        return new A\Foo();
    }
}
===file:src/A/Foo.php===
<?php

namespace App\A;

class Foo {}
===expect===
// src/A/Foo.php
2:10-2:15 → "App\\B"

// src/Relative.php
6:13-6:18 → "B\\Foo"
9:28-9:33 → "B\\Foo"
11:19-11:34 → "B\\Foo"

// src/Shadowed.php
8:28-8:33 → "\\App\\B\\Foo"
10:19-10:24 → "\\App\\B\\Foo"
