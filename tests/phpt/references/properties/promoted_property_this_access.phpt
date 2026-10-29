===description===
`$this->prop` inside a method must be returned alongside external `->prop`
accesses and the constructor param declaration when cursor is on a promoted
constructor property.
===request===
references include_declaration
===file===
<?php
class Person {
    public function __construct(public readonly string $na$0me) {}
    //                                                  ^^^^ ref
    public function greet(): string {
        return $this->name;
        //            ^^^^ ref
    }
}
$p = new Person('Alice');
echo $p->name;
//       ^^^^ ref
