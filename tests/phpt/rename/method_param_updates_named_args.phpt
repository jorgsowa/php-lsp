===allow-invalid-php===
===request===
rename who
===file:a.php===
<?php
class Greeter {
    /** @psalm-param non-empty-string $name */
    //                                ^^^^^ rename
    public function greet(string $na$0me): string { return $name; }
    //                           ^^^^^ rename
    //                                                   ^^^^^ rename
    public static function make(string $name): void {}
}

===file:b.php===
<?php
$g = new Greeter();
$g->greet(name: 'x');
//        ^^^^ rename
Greeter::make(name: 'y');
