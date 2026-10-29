===file===
<?php
function _wrap(): void {
    nonexistent_fn();
//  ^^^^^^^^^^^^^^^^ error: nonexistent_fn
}
