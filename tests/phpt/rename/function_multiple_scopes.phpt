===description===
Rename multiple occurrences of the same function in different scopes.
===allow-invalid-php===
===request===
rename handle
===file===
<?php
function process$0() { process(); }
//       ^^^^^^^ rename
//                   ^^^^^^^ rename
if (true) { process(); }
//          ^^^^^^^ rename
while (true) { process(); break; }
//             ^^^^^^^ rename
