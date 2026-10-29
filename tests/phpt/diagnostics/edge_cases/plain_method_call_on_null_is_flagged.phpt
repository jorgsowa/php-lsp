===description===
Contrast: the same definitely-null value WITHOUT the nullsafe operator
must still be flagged, proving the tests above exercise real suppression
rather than an analyzer that never checks null method/property access.
===file===
<?php
class Box { public function value(): int { return 1; } }
function _wrap(): void {
    $box = null;
    $box->value();
//  ^^^^^^^^^^^^^ error: Cannot call method value() on null
}
