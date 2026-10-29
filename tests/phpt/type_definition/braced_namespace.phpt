===description===
Braced namespace form: both the calling file and the target class use
`namespace Foo { ... }` syntax.
===request===
type_definition
===file:src/Model/Report.php===
<?php
namespace App\Model {
    class Report {}
}

===file:src/Service/Report.php===
<?php
namespace App\Service {
    class Report {}
    //    ^^^^^^ type
}

===file:src/Runner.php===
<?php
namespace App\Service {
    function run(Report $r$0): void {}
}
