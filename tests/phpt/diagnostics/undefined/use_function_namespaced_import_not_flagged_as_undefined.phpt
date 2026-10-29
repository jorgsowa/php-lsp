===description===
`use function` importing a namespaced function (the common Laravel/
Symfony helper-file pattern: `function_exists()`-guarded declarations
inside a namespace, imported unqualified elsewhere) must not be flagged
`UndefinedFunction`. No prior test covered `use function` resolution at all.
===file===
<?php
namespace App\Helpers;
if (! function_exists('App\Helpers\greet')) {
    function greet($name) {
        return $name;
    }
}
namespace App;
use function App\Helpers\greet;
function run() {
    return greet("world");
}
