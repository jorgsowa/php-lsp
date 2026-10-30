===description===
Moving several files in one request removes each of their group-use members
together, keeps the rest, and imports every moved class.
===request===
will_rename_files src/M/Two.php src/N/Two.php src/M/Inner/Three.php src/N/Inner/Three.php
===disk:composer.json===
{"autoload": {"psr-4": {"App\\": "src/"}}}
===disk:src/M/Base.php===
<?php

namespace App\M;

class Base {}
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

use App\M\{Two, Base, Inner\Three};

class User
{
    public function m(Two $two, Base $base, Three $three): void {}
}
===expect===
// src/C/User.php
4:11-4:16 → ""
4:20-4:33 → ""
5:0-5:0 → "use App\\N\\Two;\nuse App\\N\\Inner\\Three;\n"

// src/M/Inner/Three.php
2:10-2:21 → "App\\N\\Inner"

// src/M/Two.php
2:10-2:15 → "App\\N"
