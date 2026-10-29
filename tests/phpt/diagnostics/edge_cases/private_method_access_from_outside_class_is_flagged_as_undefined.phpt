===description===
Companion to the two checks above: private/protected *method* calls from
outside the class are rejected too — just mislabeled, since an
inaccessible method fails lookup and reports as `UndefinedMethod` rather
than a dedicated inaccessibility diagnostic.
===file===
<?php
class Vault {
    private function secret(): string { return 's'; }
}
function test(Vault $v): string {
    return $v->secret();
//         ^^^^^^^^^^^^ error: secret
}
