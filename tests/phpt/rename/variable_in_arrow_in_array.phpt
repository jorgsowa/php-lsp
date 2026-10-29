===description===
Edge case: arrow function in array passed as argument.
===allow-invalid-php===
===request===
rename $factor
===file===
<?php
function process(): void {
    $multiplier$0 = 2;
//  ^^^^^^^^^^^ rename
    $mappers = [
        fn($x) => $x * $multiplier,
        //             ^^^^^^^^^^^ rename
        fn($y) => $y + $multiplier,
        //             ^^^^^^^^^^^ rename
    ];
    echo $multiplier;
    //   ^^^^^^^^^^^ rename
}
