===file===
<?php
class Box { public int $value = 0; }
function _wrap(): void {
    $box = null;
    echo $box->value;
//       ^^^^^^^^^^^ error: Cannot access property $value on null
}
