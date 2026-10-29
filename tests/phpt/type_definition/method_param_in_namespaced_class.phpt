===description===
Type definition on a typed parameter inside a namespaced class method.
===request===
type_definition
===file:src/Model/Product.php===
<?php
namespace App\Model;
class Product {}
//    ^^^^^^^ type

===file:src/Service/Cart.php===
<?php
namespace App\Service;
use App\Model\Product;
class Cart {
    public function addItem(Product $item$0): void {}
}
