===description===
Cursor on a property *access* site — refs must find all `->propName`
accesses and the declaration, but not a method with the same name.
===request===
references include_declaration
===file===
<?php
class Cart {
    public int $total = 0;
    //          ^^^^^ def
    public function total(): int { return $this->total; }
    //                                           ^^^^^ ref
}
$c = new Cart();
$c->to$0tal;
//  ^^^^^ ref
$c->total += 5;
//  ^^^^^ ref
