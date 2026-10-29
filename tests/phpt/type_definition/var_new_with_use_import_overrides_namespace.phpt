===description===
`$var = new Class()` in a namespace with a `use` import: the import overrides
the namespace prefix, so $var should resolve to the imported class.
===request===
type_definition
===file:src/Model/Invoice.php===
<?php
namespace App\Model;
class Invoice {}
//    ^^^^^^^ type

===file:src/Service/Invoice.php===
<?php
namespace App\Service;
class Invoice {}

===file:src/Billing/Creator.php===
<?php
namespace App\Billing;
use App\Model\Invoice;
$inv = new Invoice();
$inv$0->total();
