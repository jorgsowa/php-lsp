===description===
`$obj?->prop` must be returned alongside `$obj->prop` and the constructor
param declaration when searching refs on a promoted constructor property.
===request===
references include_declaration
===file===
<?php
class Config {
    public function __construct(public readonly string $ke$0y) {}
    //                                                  ^^^ ref
}
$c = new Config('x');
echo $c->key;
//       ^^^ ref
echo $c?->key;
//        ^^^ ref
