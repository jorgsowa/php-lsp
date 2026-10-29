===allow-invalid-php===
===request===
references include_declaration
===file:main.php===
<?php
use App\Services\OldSe$0rvice as Service;
//  ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ ref

===file:App/Services/OldService.php===
<?php
namespace App\Services;
class OldService {}
//    ^^^^^^^^^^ def
