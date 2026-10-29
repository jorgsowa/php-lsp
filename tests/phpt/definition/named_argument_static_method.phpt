===description===
Definition on a named argument label jumps to the matching parameter of the called static method.
===request===
definition
===file:src/Circle.php===
<?php
namespace App;
class Circle {
    public static function make(float $radius): static { return new static(); }
//                                    ^^^^^^^ def
}
===file===
<?php
use App\Circle;
Circle::make(rad$0ius: 2.0);
