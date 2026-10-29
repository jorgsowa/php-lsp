===allow-invalid-php===
===request===
references include_declaration
===file===
<?php
function makeEx$0ception(): \Exception { return new \Exception(); }
//       ^^^^^^^^^^^^^ def
throw makeException();
//    ^^^^^^^^^^^^^ ref
