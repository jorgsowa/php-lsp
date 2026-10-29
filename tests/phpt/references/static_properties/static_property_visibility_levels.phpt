===description===
Test static properties with different visibility modifiers
===request===
references include_declaration
===file===
<?php
class AccessControl {
    public static string $public$0_val = '';
    //                    ^^^^^^^^^^ def
    protected static string $protected_val = '';
    private static string $private_val = '';
}

class External {
    public function test(): void {
        $val = AccessControl::$public_val;
        //                     ^^^^^^^^^^ ref
    }
}

class Internal extends AccessControl {
    public function test(): void {
        echo self::$protected_val;
    }
}
