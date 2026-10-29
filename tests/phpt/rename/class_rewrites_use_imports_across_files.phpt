===description===
Cross-file companion to `rename_class_rewrites_use_import_in_same_file`:
renaming the class in one file must rewrite both the `use` import segment
and short-name expression sites in dependents. Snapshot pinned so the
merged AST walker can't silently drop either category.
===allow-invalid-php===
===request===
rename Gadget
===file:src/Widget.php===
<?php
namespace App;
class Wid$0get {}
//    ^^^^^^ rename

===file:src/a.php===
<?php
use App\Widget;
//      ^^^^^^ rename
$x = new Widget();
//       ^^^^^^ rename
$is = $x instanceof Widget;
//                  ^^^^^^ rename

===file:src/b.php===
<?php
use App\Widget;
//      ^^^^^^ rename
$y = new Widget();
//       ^^^^^^ rename
