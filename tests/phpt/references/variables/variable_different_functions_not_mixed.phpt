===description===
References in `first()` must not include the `$result` in `second()`.
===request===
references include_declaration
===file===
<?php
function first(): void {
    $result$0 = 1;
//  ^^^^^^^ def
    echo $result;
//       ^^^^^^^ ref
}
function second(): void {
    $result = 2;
    echo $result;
}
