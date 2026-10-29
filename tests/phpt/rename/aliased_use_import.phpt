===description===
Edge case: aliased use imports must rename the alias, not the original class name.
This is critical: renaming by alias should only affect that alias, not the class itself.
===allow-invalid-php===
===request===
rename Reporter
===file===
<?php
use App\Logger as Log;
//                ^^^ rename
$l = new Log$0();
//       ^^^ rename
