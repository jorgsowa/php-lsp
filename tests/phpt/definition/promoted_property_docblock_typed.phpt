===description===
Untyped promoted param with only a `@param` docblock resolves to the param,
not to an unrelated class with the same property name.
===request===
definition
===file:service.php===
<?php
class Service {
    /** @param object $repo */
    public function __construct(private $repo) {}
    //                                   ^^^^ def
    public function run(): void { $this->re$0po; }
}

===file:other.php===
<?php
class Other {
    public object $repo;
}
