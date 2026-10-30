===description===
A moved class file drops its imports of classes in the new namespace, unless
aliased, while importing names from its old namespace.
===request===
will_rename_files src/A/Foo.php src/B/Foo.php
===disk:composer.json===
{"autoload": {"psr-4": {"App\\": "src/"}}}
===disk:src/A/Base.php===
<?php

namespace App\A;

class Base {}
===file:src/A/Foo.php===
<?php

namespace App\A;

use App\B\{One, Two};
use App\B\{Three, Sub\Kept, Four};
use App\B\Aliased as Other;
use App\B\Already;

class Foo extends Base
{
    public function m(One $a, Two $b, Three $c, Kept $d, Four $e, Other $f, Already $g): void {}
}
===expect===
// src/A/Foo.php
2:10-2:15 → "App\\B"
4:0-5:0 → ""
5:11-5:18 → ""
5:26-5:32 → ""
7:0-8:0 → ""
8:0-8:0 → "use App\\A\\Base;\n"
