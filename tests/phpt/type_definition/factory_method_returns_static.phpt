===description===
Factory method with explicit class return: `Factory::make()` returns `static`.
===request===
type_definition
===file===
<?php
class Factory {
//    ^^^^^^^ type
    public static function make(): static { return new static(); }
}
$f = Factory::make();
$f$0->process();
