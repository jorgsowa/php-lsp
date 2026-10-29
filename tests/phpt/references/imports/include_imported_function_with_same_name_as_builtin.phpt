===allow-invalid-php===
===request===
references include_declaration
===file:main.php===
<?php
use function App\str$0len;
//           ^^^^^^^^^^ ref

===file:App/functions.php===
<?php
namespace App;
function strlen(string $value): int { return 0; }
//       ^^^^^^ def
