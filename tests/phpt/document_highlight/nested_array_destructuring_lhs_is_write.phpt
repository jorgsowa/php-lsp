===description===
Nested array destructuring: `[[$a, $b], $c] = ...` — all three variables
are write positions.
===request===
document_highlight
===file===
<?php
function foo(): void {
    [[$fi$0rst, $second], $third] = [[1, 2], 3];
//    ^^^^^^ write
    echo $first;
//       ^^^^^^ read
}
