===description===
A `use` added to a caller in a braced namespace takes the indentation of the
namespace body.
===request===
will_rename_files src/Model/User.php src/Domain/User.php
===disk:composer.json===
{"autoload": {"psr-4": {"App\\": "src/"}}}
===disk:src/Model/Repo.php===
<?php

namespace App\Model {
    class Repo
    {
        public function find(): User
        {
            return new User();
        }
    }
}
===disk:src/Model/Finder.php===
<?php

namespace App\Model {
	use Countable;

	class Finder
	{
		public function find(): User
		{
			return new User();
		}
	}
}
===file:src/Model/User.php===
<?php

namespace App\Model;

class User
{
}
===expect===
// src/Model/Finder.php
4:0-4:0 → "\tuse App\\Domain\\User;\n"

// src/Model/Repo.php
3:0-3:0 → "    use App\\Domain\\User;\n"

// src/Model/User.php
2:10-2:19 → "App\\Domain"
