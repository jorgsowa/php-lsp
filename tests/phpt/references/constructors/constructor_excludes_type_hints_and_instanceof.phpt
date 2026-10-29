===description===
__construct references must only include `new` call sites — not type hints,
`instanceof`, or `::class`. The annotation DSL implicitly asserts exclusions:
any location the server returns that isn't annotated causes a diff failure.
===request===
references include_declaration
===file===
<?php
class Order {
    public function __con$0struct(int $id) {}
    //              ^^^^^^^^^^^ def
}
$o = new Order(1);
//       ^^^^^ ref
function ship(Order $o): void {}
if ($o instanceof Order) {}
Order::class;
