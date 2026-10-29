===description===
All three occurrences of a typed param must appear, and the declaration
span must start at `$`, not at the type annotation.
===request===
references include_declaration
===file===
<?php
function process(string $val$0): string {
    //                  ^^^^ def
    $out = strtoupper($val);
    //                ^^^^ ref
    return $val;
    //     ^^^^ ref
}
