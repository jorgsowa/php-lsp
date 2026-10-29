===request===
definition
===file:service.php===
<?php
class Service {
    public function __construct(private object $repo) {}
    //                                          ^^^^ def
    public function run(): void { $this->re$0po; }
}

===file:other.php===
<?php
class Other {
    public object $repo;
}
