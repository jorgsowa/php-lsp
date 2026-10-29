===description===
Closure `use()` clause variables are included when renaming the captured variable.
===allow-invalid-php===
===request===
rename $person
===file===
<?php
function greet(): void {
    $name$0 = "Alice";
//  ^^^^^ rename
    $greeting = function() use ($name) {
    //                          ^^^^^ rename
        echo "Hello " . $name;
        //              ^^^^^ rename
    };
    echo $name;
    //   ^^^^^ rename
}
