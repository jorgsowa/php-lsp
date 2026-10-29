===description===
Foreach value variable (no key) must be found and scoped to the function.
===request===
references include_declaration
===file===
<?php
function sum(array $nums): int {
    $total = 0;
    foreach ($nums as $nu$0m) {
        //            ^^^^ def
        $total += $num;
        //        ^^^^ ref
    }
    return $total;
}
