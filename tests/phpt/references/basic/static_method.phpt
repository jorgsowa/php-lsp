===request===
references include_declaration
===file===
<?php
class Reg {
    public static function ge$0t(): void {}
    //                     ^^^ def
}
Reg::get();
//   ^^^ ref
Reg::get();
//   ^^^ ref
