===allow-invalid-php===
===request===
rename my_contains
===file===
<?php
function str_con$0tains(string $h, string $n): bool { return true; }
//       ^^^^^^^^^^^^ rename
  str_contains('a', 'b');
//^^^^^^^^^^^^ rename
