===description===
Goto-definition with cursor ON the parameter declaration itself (not in the
body) must still resolve to that same parameter.
===request===
definition
===file===
<?php
function transform(int $val$0): int {
    //                 ^^^^ def
    return $val * 2;
}
