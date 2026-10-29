===allow-invalid-php===
===request===
references include_declaration
===file===
<?php
class C {
    public int $x = C::ma$0ke();
    //                 ^^^^ ref
    public static function make(): int {
    //                     ^^^^ def
        return 0;
    }
}
