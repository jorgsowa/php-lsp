===description===
Cursor on a body-assigned variable (not a parameter) returns all occurrences
in scope; a same-named variable in another function is excluded.
===request===
references include_declaration
===file===
<?php
function build(): string {
    $out$0 = 'start';
//  ^^^^ def
    $out .= ' middle';
//  ^^^^ ref
    return $out;
//         ^^^^ ref
}
function other(): string { $out = 'x'; return $out; }
