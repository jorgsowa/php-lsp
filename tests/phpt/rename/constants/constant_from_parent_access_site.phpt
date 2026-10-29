===description===
Rename shares `resolve_reference_symbol` with the references handler —
renaming from a `parent::CONST` cursor must edit the declaration and
every usage form, proving the owner-resolution fix also benefits rename.
===request===
rename LIMIT
===file===
<?php
class Base {
    const MAX = 100;
    //    ^^^ rename

    public function limit(): int {
        return self::MAX;
        //           ^^^ rename
    }
}
class Derived extends Base {
    public function check(int $v): bool {
        return $v <= parent::MA$0X;
        //                   ^^^ rename
    }
}
$ok = Base::MAX;
//          ^^^ rename
