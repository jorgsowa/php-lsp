===allow-invalid-php===
===request===
references include_declaration
===file===
<?php
function che$0ck(): void {}
//       ^^^^^ def
if (true) { check(); }
//          ^^^^^ ref
