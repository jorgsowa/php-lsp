===description===
Bug 1: two constructors in the same file — the decl span for Beta's
__construct must point at Beta (line 5), not Alpha (line 2).
===request===
references include_declaration
===file===
<?php
class Alpha {
    public function __construct(int $x) {}
}
class Beta {
    public function __con$0struct(string $s) {}
    //              ^^^^^^^^^^^ def
}
new Alpha(1);
new Beta('x');
//  ^^^^ ref
