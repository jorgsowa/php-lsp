===description===
Bug 2: braced-namespace constructor must be found by references.
===request===
references include_declaration
===file===
<?php
namespace Shop {
    class Order {
        public function __con$0struct(int $id) {}
        //              ^^^^^^^^^^^ def
    }
}
namespace Shop {
    $o = new Order(1);
    //       ^^^^^ ref
}
