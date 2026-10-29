===description===
Cursor on a property *declaration* — refs must include the declaration
itself plus every access site, but not a same-named method call.
===request===
references include_declaration
===file===
<?php
class Order {
    public string $sta$0tus = '';
    //             ^^^^^^ def
    public function status(): string { return $this->status; }
    //                                               ^^^^^^ ref
}
$o = new Order();
$o->status;
//  ^^^^^^ ref
