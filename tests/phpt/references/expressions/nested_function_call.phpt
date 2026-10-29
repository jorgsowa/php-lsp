===allow-invalid-php===
===request===
references include_declaration
===file===
<?php
function gr$0eet(): void {}
//       ^^^^^ def
echo(greet());
//   ^^^^^ ref
