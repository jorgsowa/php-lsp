===file===
<?php
function _wrap(): void {
    $arr = null;
    echo $arr['key'];
//       ^^^^^^^^^^^ error: Cannot access array on null
}
