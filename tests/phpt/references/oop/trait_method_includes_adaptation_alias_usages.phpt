===description===
Trait adaptation clauses also refer to methods. `use Auditable { record as
audit; }` creates an alias whose calls still use the trait method body, so a
complete find-usages result from the original trait method should include
both the adaptation entry and calls through the alias.
===request===
references include_declaration
===file===
<?php
trait Auditable {
    public function rec$0ord(): void {}
    //              ^^^^^^ def
}
class Post {
    use Auditable {
        record as audit;
      //^^^^^^ ref
    }
    public function save(): void {
        $this->record();
        //     ^^^^^^ ref
        $this->audit();
        //     ^^^^^ ref
    }
}
