===request===
definition
===file===
<?php
class Greeter {
    public function sayHello(): void {}
    //              ^^^^^^^^ def
}
$g = new Greeter();
$g->sayhe$0llo();
