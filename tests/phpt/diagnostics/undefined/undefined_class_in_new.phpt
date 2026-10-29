===description===
Positive control for the above: a truly-missing same-namespace class must
still be flagged. Without this, the no-false-positive tests prove nothing.
===file===
<?php
function _wrap(): void {
    $x = new UnknownClass();
//           ^^^^^^^^^^^^ error: UnknownClass
}
