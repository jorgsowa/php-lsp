===allow-invalid-php===
===request===
references include_declaration
===file===
<?php
trait T {
    public int $x = self::in$0it();
    //                    ^^^^ ref
    public static function init(): int {
    //                     ^^^^ def
        return 0;
    }
}
