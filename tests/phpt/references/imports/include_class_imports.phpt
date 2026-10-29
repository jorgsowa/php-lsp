===allow-invalid-php===
===request===
references include_declaration
===file:main.php===
<?php
use App\Ba$0r;
//  ^^^^^^^ ref
use function App\helper;
use const App\LIMIT;

===file:App/Bar.php===
<?php
namespace App;
class Bar {}
//    ^^^ def
