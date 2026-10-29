===description===
Arrow functions auto-capture outer-scope variables; rename covers those captures.
===allow-invalid-php===
===request===
rename $result
===file===
<?php
function process(): void {
    $value$0 = 42;
//  ^^^^^^ rename
    $fn = fn() => $value + 1;
    //            ^^^^^^ rename
    echo $value;
    //   ^^^^^^ rename
}
