===allow-invalid-php===
===request===
rename $new
===file===
<?php
function outer() {
    $old = 1;
//  ^^^^ rename
    $f = function () use ($old) { $old = 2; return $o$0ld; };
    //                    ^^^^ rename
    //                            ^^^^ rename
    //                                             ^^^^ rename
}
