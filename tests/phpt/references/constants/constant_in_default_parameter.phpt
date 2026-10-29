===request===
references include_declaration
===file===
<?php
class Limits {
    const DEF$0AULT_SIZE = 100;
    //    ^^^^^^^^^^^^ def
}

function process(int $size = Limits::DEFAULT_SIZE): void {
//                                   ^^^^^^^^^^^^ ref
    echo $size;
}
