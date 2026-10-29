===file===
<?php
function wrapper(): void {
    nonexistent_fn();
//  ^^^^^^^^^^^^^^^^ error: nonexistent_fn
}
