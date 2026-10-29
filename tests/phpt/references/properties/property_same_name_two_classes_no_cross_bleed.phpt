===description===
Two classes in the same file share the same property name. References for one
must NOT include the declaration of the other — previously `str_offset` found
the first occurrence in the file, mapping both declarations to class A's span.
===request===
references include_declaration
===file===
<?php
class A {
    public int $va$0lue = 0;
    //          ^^^^^ def
    public function getA(): int { return $this->value; }
    //                                          ^^^^^ ref
}
class B {
    public int $value = 0;
    public function getB(): int { return $this->value; }
}
