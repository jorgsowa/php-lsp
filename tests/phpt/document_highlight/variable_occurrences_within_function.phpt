===request===
document_highlight
===file===
<?php
function f(): void {
    $name = 'x';
//  ^^^^^ write
    echo $na$0me;
//       ^^^^^ read
    $name .= '!';
//  ^^^^^ write
}
