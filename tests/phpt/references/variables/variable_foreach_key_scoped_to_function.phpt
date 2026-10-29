===description===
`$key` in a foreach loop shares function scope with code after the loop —
all occurrences within the function are returned, but a same-named
variable in a different function is excluded.
===request===
references include_declaration
===file===
<?php
function iter(array $items): void {
    foreach ($items as $ke$0y => $child) {
        //             ^^^^ def
        echo $key;
        //   ^^^^ ref
    }
    $key = 'reset';
//  ^^^^ ref
}
function other(): void { $key = 'unrelated'; }
