===request===
type_definition
===file:src/Mailer.php===
<?php
namespace Vendor;
class Mailer {}
//    ^^^^^^ type

===file:src/main.php===
<?php
use Vendor\Mailer;
$m = new Mailer();
$m$0->send();
