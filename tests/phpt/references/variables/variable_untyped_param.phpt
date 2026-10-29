===description===
An untyped (plain) parameter `function foo($x)` must be found correctly;
`p.span` already starts at `$` so no narrowing is needed.
===request===
references include_declaration
===file===
<?php
function double($n$0): int {
    //          ^^ def
    return $n * 2;
    //     ^^ ref
}
function triple($n): int { return $n * 3; }
