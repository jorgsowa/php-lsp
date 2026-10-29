===description===
A `new X(...)` call site living inside a PHP 8 attribute's argument list
(e.g. PHPUnit's `#[TestWith([new Spread(...)])]`) must be found by
find-references on `X`, exactly like an identical `new X(...)` in regular
code. This was a confirmed gap against a real ~15K-file codebase: the
plain-code instantiation was found, the attribute-nested one silently was
not. Regression pin — this currently passes.
===request===
references include_declaration
===file===
<?php
class Spread$0 {
//    ^^^^^^ def

    public function __construct(public string $id) {}
}

class IntermediateJson {
    public function __construct(public array $spreads) {}
}

function plain(): IntermediateJson {
    return new IntermediateJson([new Spread('s0')]);
//                                   ^^^^^^ ref
}

#[TestWith([new Spread('s1')])]
//              ^^^^^^ ref
function attributeArg(): void {}
