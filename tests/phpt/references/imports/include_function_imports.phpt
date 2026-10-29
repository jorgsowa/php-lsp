===allow-invalid-php===
===request===
references include_declaration
===file:main.php===
<?php
use function App\he$0lper;
//           ^^^^^^^^^^ ref

===file:App/functions.php===
<?php
namespace App;
function helper() {}
//       ^^^^^^ def
