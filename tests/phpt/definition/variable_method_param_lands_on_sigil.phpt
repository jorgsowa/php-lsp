===description===
Goto-definition inside a method body with a typed parameter must land on
`$param`, not the type annotation.
===request===
definition
===file===
<?php
class Greeter {
    public function greet(string $name): string {
        //                       ^^^^^ def
        return $name$0;
    }
}
