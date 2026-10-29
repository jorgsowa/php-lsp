===description===
Method parameter references must be scoped to the method and not include
the same-named parameter in a sibling method.
===request===
references include_declaration
===file===
<?php
class Calc {
    public function add(int $a$0, int $b): int {
        //                  ^^ def
        return $a + $b;
        //     ^^ ref
    }
    public function mul(int $a, int $b): int { return $a * $b; }
}
