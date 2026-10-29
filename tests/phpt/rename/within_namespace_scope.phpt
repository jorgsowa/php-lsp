===description===
Renaming a class is FQN-aware; same-named symbols in other namespaces are not affected.
===allow-invalid-php===
===request===
rename Reporter
===file===
<?php
namespace App;
class Logger$0 {}
//    ^^^^^^ rename
function create() {
    $l = new Logger();
    //       ^^^^^^ rename
}
