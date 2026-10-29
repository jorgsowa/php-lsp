===description===
`Base::boot` is protected. It is NOT narrowed (its complete scope needs
the transitive subtype set, only known once indexing finishes), so the
full-scope search must still find calls in both the declaring class and a
subclass in another file.
===request===
references include_declaration
===file:src/Base.php===
<?php
namespace App;
class Base {
    protected function bo$0ot(): void {}
    //                 ^^^^ def
    public function init(): void {
        $this->boot();
        //     ^^^^ ref
    }
}

===file:src/Child.php===
<?php
namespace App;
class Child extends Base {
    public function start(): void {
        $this->boot();
        //     ^^^^ ref
    }
}
