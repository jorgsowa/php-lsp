===description===
Searching references from a property *access* site (`$this->prop`) must
behave the same as searching from the constructor param declaration —
finding all property accesses, not method calls.
===request===
references include_declaration
===file===
<?php
class Cart {
    public function __construct(private object $item) {}
    //                                          ^^^^ ref
    public function total(): void { $this->it$0em; }
    //                                     ^^^^ ref
    public function describe(): void { $this->item; }
    //                                        ^^^^ ref
}
