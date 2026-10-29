===description===
Factory method returns: `Foo::create()` should resolve to Foo's type.
This is a common pattern where static factory methods return instances of their class.
===request===
type_definition
===file===
<?php
class Builder {
//    ^^^^^^^ type
    public static function create(): self { return new self(); }
}
$b = Builder::create();
$b$0->build();
