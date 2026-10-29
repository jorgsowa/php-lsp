===allow-invalid-php===
===request===
references include_declaration
===file:main.php===
<?php
namespace App;
use App\Services\Log$0ger;
//  ^^^^^^^^^^^^^^^^^^^ ref

===file:App/Services/Logger.php===
<?php
namespace App\Services;
class Logger {}
//    ^^^^^^ def
