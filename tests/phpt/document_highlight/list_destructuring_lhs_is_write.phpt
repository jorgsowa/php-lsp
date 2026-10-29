===description===
`list($a, $b) = expr` is the classical destructuring form; the parser
normalises it to the same array-destructuring AST as `[$a, $b] = expr`.
===request===
document_highlight
===file===
<?php
function foo(): void {
    list($ke$0y, $val) = ['k', 'v'];
//       ^^^^ write
    echo $key;
//       ^^^^ read
}
