===description===
Goto-definition for a typed parameter with a default value must land on
`$param`, not on the type or the default.
===request===
definition
===file===
<?php
function greet(string $name = 'World'): string {
    //                ^^^^^ def
    return 'Hello ' . $name$0;
}
