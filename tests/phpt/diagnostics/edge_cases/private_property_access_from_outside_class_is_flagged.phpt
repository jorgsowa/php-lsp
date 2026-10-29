===description===
Reading a `private` property from outside its class is flagged, matching
the class-constant and method-call visibility checks.
===file===
<?php
class Vault {
    private int $secret = 0;
}
function test(Vault $v): int {
    return $v->secret;
//             ^^^^^^ error: secret
}
