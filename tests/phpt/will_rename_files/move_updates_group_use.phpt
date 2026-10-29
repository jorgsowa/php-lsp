===description===
Moving a class file respells its group-use member when the group prefix still
applies, else moves it to its own `use`.
===request===
will_rename_files src/A/Foo.php src/B/Foo.php
===disk:composer.json===
{"autoload": {"psr-4": {"App\\": "src/"}}}
===disk:src/A/Base.php===
<?php

namespace App\A;

class Base {}
===disk:src/C/Mixed.php===
<?php

namespace App\C;

use App\A\{Base, Foo as G};

class Mixed
{
    public function m(Base $b, G $g): void {}
}
===disk:src/C/Only.php===
<?php

namespace App\C;

use App\A\{Foo};

class Only
{
    public function m(Foo $f): void {}
}
===disk:src/C/Shared.php===
<?php

namespace App\C;

use App\{A\Foo, C\Other};

class Shared
{
    public function m(Foo $f): void {}
}
===disk:src/C/Multiline.php===
<?php

namespace App\C;

use App\A\{
    Base,
    Foo,
};

class Multiline
{
    public function m(Base $b, Foo $f): void {}
}
===file:src/A/Foo.php===
<?php

namespace App\A;

class Foo {}
===expect===
// src/A/Foo.php
2:10-2:15 → "App\\B"

// src/C/Mixed.php
4:15-4:25 → ""
5:0-5:0 → "use App\\B\\Foo as G;\n"

// src/C/Multiline.php
5:8-6:7 → ""
8:0-8:0 → "use App\\B\\Foo;\n"

// src/C/Only.php
4:0-4:16 → "use App\\B\\Foo;"

// src/C/Shared.php
4:9-4:14 → "B\\Foo"
