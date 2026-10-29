===description===
Calling a method that does not exist produces UndefinedMethod.
===file===
<?php
class Greeter {
    public function hello(): void {}
}
function test(Greeter $g): void {
    $g->doesNotExist();
//  ^^^^^^^^^^^^^^^^^^ error: doesNotExist
}
