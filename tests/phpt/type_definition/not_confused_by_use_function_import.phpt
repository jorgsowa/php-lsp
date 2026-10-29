===description===
`use function` imports must not pollute the class-import map: a type hint
`format $x` where `format` also appears in `use function Lib\format` should
resolve to the same-namespace class `App\format`, not to `Lib\format`.
===request===
type_definition
===file:main.php===
<?php
namespace App;
use function Lib\format;

function go(format $x$0): void {}

===file:format.php===
<?php
namespace App;
class format {}
//    ^^^^^^ type
