===allow-invalid-php===
===request===
rename $new
===file===
<?php
function f(int ...$old) {
//                ^^^^ rename
    return function () use ($old) { return $o$0ld; };
    //                      ^^^^ rename
    //                                     ^^^^ rename
}
