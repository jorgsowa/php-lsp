===request===
document_highlight
===file===
<?php
class Calc {
    public static function add$0() { return 1 + 2; }
    //                     ^^^ ref
}
Calc::add();
//    ^^^ ref
