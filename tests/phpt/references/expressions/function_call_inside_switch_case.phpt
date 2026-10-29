===allow-invalid-php===
===request===
references include_declaration
===file===
<?php
function ti$0ck(): void {}
//       ^^^^ def
switch ($x) {
    case 1: tick(); break;
    //      ^^^^ ref
}
