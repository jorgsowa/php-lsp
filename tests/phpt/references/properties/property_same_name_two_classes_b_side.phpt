===description===
Same scenario from the other class — cursor on B's property must only show B's
declaration and B's access site, not A's declaration or A's access.
===request===
references include_declaration
===file===
<?php
class A {
    public int $value = 0;
    public function getA(): int { return $this->value; }
}
class B {
    public int $va$0lue = 0;
    //          ^^^^^ def
    public function getB(): int { return $this->value; }
    //                                          ^^^^^ ref
}
