===description===
Rename variable across multiple functions (comprehensive coverage).
Verifies that rename works correctly with deeply nested scopes.
===allow-invalid-php===
===request===
rename $y
===file===
<?php
function outer() {
    $x$0 = 1;
  //^^ rename
    function inner() {
        $x = 2;
    }
    echo $x;
    //   ^^ rename
}
