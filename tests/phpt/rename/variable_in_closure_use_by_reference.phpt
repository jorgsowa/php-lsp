===description===
Edge case: closure use() clause with reference binding.
===allow-invalid-php===
===request===
rename $total
===file===
<?php
function counter(): void {
    $count$0 = 0;
//  ^^^^^^ rename
    $increment = function() use (&$count) {
    //                            ^^^^^^ rename
        $count++;
    //  ^^^^^^ rename
    };
    $increment();
    echo $count;
    //   ^^^^^^ rename
}
