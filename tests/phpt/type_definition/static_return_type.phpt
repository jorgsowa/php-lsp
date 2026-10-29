===description===
Parameter with Factory type resolves correctly (test previously ignored for wrong reason).
===request===
type_definition
===file===
<?php
class Factory {
//    ^^^^^^^ type
    public function create(): static { return new static(); }
    public function use_it(Factory $f$0): void {}
}
