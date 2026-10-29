===description===
Top-level declarations report their namespace as containerName; members report their class.
===request===
workspace_symbols
===file===
<?php
namespace App\Models;
class User { public function save() {} }
function helper() {}
===file:braced.php===
<?php
namespace Lib {
    interface Shape {}
}
namespace {
    function global_fn() {}
}
===expect===
Class       User @ main.php:2 in App\Models
Function    global_fn @ braced.php:5
Function    helper @ main.php:3 in App\Models
Interface   Shape @ braced.php:2 in Lib
Method      save @ main.php:2 in User
