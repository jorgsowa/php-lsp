===request===
references
===file:src/functions.php===
<?php
namespace Psy;

if (!\function_exists('Psy\\info')) {
//                     ^^^^^^^^^ ref
    function in$0fo(): array { return []; }
}

===file:src/Shell.php===
<?php
namespace Psy;

class Shell {
    public function run(): array {
        $label = 'info';
        return info();
        //     ^^^^ ref
    }
}
