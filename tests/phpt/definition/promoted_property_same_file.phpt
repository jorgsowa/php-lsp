===description===
go-to-definition on a promoted constructor property should jump to the
parameter declaration, not to an unrelated class that happens to have a
property with the same name.
===request===
definition
===file===
<?php
class Service {
    public function __construct(private object $repo) {}
    //                                          ^^^^ def
    public function run(): void { $this->re$0po; }
}
