===description===
Renaming a class rewrites the matching `use` import in addition to call sites.
===allow-invalid-php===
===request===
rename Gadget
===file===
<?php
use Vendor\Lib\Widget;
//             ^^^^^^ rename
$a = new Wid$0get();
//       ^^^^^^ rename
$b = new Widget();
//       ^^^^^^ rename
