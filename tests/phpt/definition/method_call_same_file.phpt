===request===
definition
===file===
<?php
class Greeter {
    public function hello(): string { return 'hi'; }
    //              ^^^^^ def
}
$g = new Greeter();
$g->hel$0lo();
