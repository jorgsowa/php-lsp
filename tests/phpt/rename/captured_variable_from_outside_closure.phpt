===description===
A closure's captured variable is renamed in its body whether the rename
starts outside or inside the closure.
===allow-invalid-php===
===request===
rename $new
===file===
<?php
function outer() {
    $ol$0d = 1;
//  ^^^^ rename
    $f = function () use ($old) { return $old; };
    //                    ^^^^ rename
    //                                   ^^^^ rename
    $g = function () { return $old; };
}
