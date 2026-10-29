===request===
references include_declaration
===file===
<?php
class Abstract$0Base {}
//    ^^^^^^^^^^^^ def

#[AbstractBase]
//^^^^^^^^^^^^ ref
abstract class Handler {
    abstract public function handle(): void;
}
