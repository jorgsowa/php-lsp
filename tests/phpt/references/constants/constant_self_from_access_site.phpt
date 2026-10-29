===description===
Cursor on `self::CONST` usage inside the same class must find declaration
and all usages (both `self::` and `ClassName::` forms).
===request===
references include_declaration
===file===
<?php
class Config {
    const MAX = 100;
    //    ^^^ def

    public function check(int $v): bool {
        return $v <= self::MA$0X;
        //                 ^^^ ref
    }
}
$ok = Config::MAX;
//            ^^^ ref
