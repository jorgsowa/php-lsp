===file===
<?php
function _wrap(): void {
    one_undefined();
//  ^^^^^^^^^^^^^^^ error: one_undefined
    two_undefined();
//  ^^^^^^^^^^^^^^^ error: two_undefined
}
