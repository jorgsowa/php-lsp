===description===
Edge case: Compound use statement with multiple imports.
`use A, B;` when renaming A should only affect A, not B.
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
