===description===
`$var = new Class()` in a namespace resolves to the class in the same namespace.
===request===
type_definition
===file:src/Model/Order.php===
<?php
namespace App\Model;
class Order {}

===file:src/Service/Order.php===
<?php
namespace App\Service;
class Order {}
//    ^^^^^ type

===file:src/Service/Processor.php===
<?php
namespace App\Service;
$order = new Order();
$order$0->process();
