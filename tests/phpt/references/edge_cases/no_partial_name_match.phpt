===description===
`greet` must not include occurrences of `greeting`.
===allow-invalid-php===
===request===
references include_declaration
===file===
<?php
function gr$0eet(): void {}
//       ^^^^^ def
function greeting(): void {}
  greet();
//^^^^^ ref
greeting();
