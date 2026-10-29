===request===
definition
===file===
<?php
trait Greeting {
//    ^^^^^^^^ def
    public function sayHello(string $name): string { return ""; }
}
class Greeter {
    use $0Greeting;
}
