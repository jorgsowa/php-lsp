===description===
`static $x = val` declares and initialises a persistent local; the declaration
must appear as a WRITE in highlights (previously it was missing entirely because
`walk_stmt` for `StaticVar` only visited the default expression, not the name).
===request===
document_highlight
===file===
<?php
function counter(): int {
    static $co$0unt = 0;
//         ^^^^^^ write
    $count++;
//  ^^^^^^ write
    return $count;
//         ^^^^^^ read
}
