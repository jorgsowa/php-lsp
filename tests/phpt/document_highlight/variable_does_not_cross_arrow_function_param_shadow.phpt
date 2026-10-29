===description===
An arrow function's own parameter shadows an outer variable of the same
name — the two must not be merged into one highlight group, even though
arrow functions otherwise auto-capture (and so are normally traversed).
===request===
document_highlight
===file===
<?php
function foo() {
    $x$0 = 1;
//  ^^ write
    $inner = fn($x) => $x + 1;
    echo $x;
//       ^^ read
}
