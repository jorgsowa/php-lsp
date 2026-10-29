===request===
references include_declaration
===file===
<?php
class BaseStorage {
    protected static array $coun$0t = [];
    //                      ^^^^^ def
}

class ExtendedStorage extends BaseStorage {
    public static function increment(): void {
        parent::$count[] = 1;
        //       ^^^^^ ref
    }

    public static function reset(): void {
        parent::$count = [];
        //       ^^^^^ ref
    }
}
