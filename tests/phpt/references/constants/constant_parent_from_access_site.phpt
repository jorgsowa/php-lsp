===description===
Cursor on `parent::CONST` usage (not the declaration) must find the
declaration and all usages (`self::`, `parent::`, `ClassName::` forms) —
the `parent::` counterpart to `constant_self_from_access_site`, exercising
`resolve_reference_symbol`'s `class_before_double_colon` owner resolution
for the `"parent"` case the same way it already does for `self`/`static`.
===request===
references include_declaration
===file===
<?php
class Base {
    const MAX = 100;
    //    ^^^ def

    public function limit(): int {
        return self::MAX;
        //           ^^^ ref
    }
}
class Derived extends Base {
    public function check(int $v): bool {
        return $v <= parent::MA$0X;
        //                   ^^^ ref
    }
}
$ok = Base::MAX;
//          ^^^ ref
