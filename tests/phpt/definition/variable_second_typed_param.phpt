===description===
Goto-definition for the second of several typed params must land on the
correct `$param`, not on the first one.
===request===
definition
===file===
<?php
function add(int $a, int $b): int {
    //                   ^^ def
    return $a + $b$0;
}
