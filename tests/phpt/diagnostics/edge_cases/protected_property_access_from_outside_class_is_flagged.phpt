===description===
Same check as above, for `protected` instead of `private`.
===file===
<?php
class Vault {
    protected int $balance = 0;
}
function test(Vault $v): int {
    return $v->balance;
//             ^^^^^^^ error: balance
}
