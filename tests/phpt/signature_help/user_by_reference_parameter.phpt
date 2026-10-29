===description===
Signature help on a user function places `&` between a by-reference parameter's type and name.
===request===
signature_help
===file===
<?php
function byref(array &$a, &...$rest): void {}
byref($0);
===expect===
▶ byref(array &$a, &...$rest)  @param0
