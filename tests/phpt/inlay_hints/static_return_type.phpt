===description===
`static`/`self` return types are labelled with the class name, as hover shows it.
===request===
inlay_hints
===file===
<?php
namespace App;
class Circle {
    public static function make(): static { return new static(); }
    public function copy(): self { return $this; }
}
$c = Circle::make();
$d = $c->copy();
===expect===
6:19 : App\Circle [type]
7:15 : App\Circle [type]
