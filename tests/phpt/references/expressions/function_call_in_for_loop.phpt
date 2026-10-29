===allow-invalid-php===
===request===
references include_declaration
===file===
<?php
function ti$0ck(): void {}
//       ^^^^ def
for (tick(); $i < 10; tick()) {}
//   ^^^^ ref
//                    ^^^^ ref
