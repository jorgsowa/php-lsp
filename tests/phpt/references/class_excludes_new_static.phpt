===description===
Late-static-bound `new static` and `static::` don't name the class, so they aren't references to it.
===request===
references include_declaration
===file===
<?php
class Shape {
//    ^^^^^ def
    public static function create(): static { return new static(); }
    public static function again(): static { return static::create(); }
}
class Circle extends Shape {}
//                   ^^^^^ ref
$s = new Sh$0ape();
//       ^^^^^ ref
