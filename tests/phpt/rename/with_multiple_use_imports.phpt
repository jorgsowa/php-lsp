===description===
Edge case: multiple imports in a single use statement must all be updated.
Test: use A\Foo, B\Bar; when renaming Foo
===allow-invalid-php===
===request===
rename Reporter
===file===
<?php
use App\Logger, App\Parser;
//      ^^^^^^ rename
$l = new Logger$0();
//       ^^^^^^ rename
$p = new Parser();
