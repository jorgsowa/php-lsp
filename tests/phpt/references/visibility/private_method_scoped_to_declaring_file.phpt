===description===
`A::help` is private; its references live only in A's file. A same-named
private method on an unrelated class B in another file must be ignored.
===request===
references include_declaration
===file:src/A.php===
<?php
namespace App;
class A {
    private function he$0lp(): void {}
    //               ^^^^ def
    public function run(): void {
        $this->help();
        //     ^^^^ ref
    }
}

===file:src/B.php===
<?php
namespace App;
class B {
    private function help(): void {}
    public function go(): void {
        $this->help();
    }
}
