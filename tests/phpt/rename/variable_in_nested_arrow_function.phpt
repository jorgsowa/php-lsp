===description===
Edge case: arrow function with multiple captures and nested operations.
===allow-invalid-php===
===request===
rename $initial
===file===
<?php
function compute(): void {
    $base$0 = 10;
//  ^^^^^ rename
    $offset = 5;
    $calc = fn() => fn() => $base + $offset;
    //                      ^^^^^ rename
    echo $base;
    //   ^^^^^ rename
}
