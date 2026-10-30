===description===
Moving a folder splits a group use whose members all move into one import each.
===request===
will_rename_files src/M src/N
===disk:composer.json===
{"autoload": {"psr-4": {"App\\": "src/"}}}
===disk:src/M/Two.php===
<?php

namespace App\M;

class Two {}
===disk:src/M/Inner/Three.php===
<?php

namespace App\M\Inner;

class Three {}
===file:src/C/User.php===
<?php

namespace App\C;

use App\M\{Two, Inner\Three};

class User
{
    public function m(Two $two, Three $three): void {}
}
===expect===
// src/C/User.php
4:0-4:29 → "use App\\N\\Two;\nuse App\\N\\Inner\\Three;"

// src/M/Inner/Three.php
2:10-2:21 → "App\\N\\Inner"

// src/M/Two.php
2:10-2:15 → "App\\N"
