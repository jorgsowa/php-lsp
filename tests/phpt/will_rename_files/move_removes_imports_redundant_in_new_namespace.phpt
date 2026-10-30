===description===
Callers already in the new namespace drop their import of the moved class,
unless it binds another name.
===request===
will_rename_files src/A/Foo.php src/B/Foo.php
===disk:composer.json===
{"autoload": {"psr-4": {"App\\": "src/"}}}
===disk:src/A/Base.php===
<?php

namespace App\A;

class Base {}
===disk:src/B/Plain.php===
<?php

namespace App\B;

use App\A\Foo;

class Plain
{
    public function m(Foo $f): void {}
}
===disk:src/B/Group.php===
<?php

namespace App\B;

use App\A\{Base, Foo};

class Group
{
    public function m(Base $b, Foo $f): void {}
}
===disk:src/B/Aliased.php===
<?php

namespace App\B;

use App\A\Foo as Bar;

class Aliased
{
    public function m(Bar $f): void {}
}
===file:src/A/Foo.php===
<?php

namespace App\A;

class Foo {}
===expect===
// src/A/Foo.php
2:10-2:15 → "App\\B"

// src/B/Aliased.php
4:4-4:13 → "App\\B\\Foo"

// src/B/Group.php
4:15-4:20 → ""

// src/B/Plain.php
4:0-6:0 → ""
