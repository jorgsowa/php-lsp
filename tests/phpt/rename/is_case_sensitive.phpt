===description===
Rename must be case-sensitive and not match names that differ only in case.
===allow-invalid-php===
===request===
rename verify
===file===
<?php
function test() {}
//       ^^^^ rename
function Test() {}
  tes$0t();
//^^^^ rename
