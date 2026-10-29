===description===
The keyword bail-out must not shadow the legitimate `self::`/`static::`
resolution path it sits right next to in `symbol_kind_at`: `self` and
`static` are themselves reserved words, but immediately before `::` they
must still resolve through to the method they qualify.
===allow-invalid-php===
===request===
references include_declaration
===file===
<?php
class C {
    public static function ma$0ke(): static { return new static(); }
    //                     ^^^^ def
    public static function via(): static { return self::make(); }
    //                                                  ^^^^ ref
}
