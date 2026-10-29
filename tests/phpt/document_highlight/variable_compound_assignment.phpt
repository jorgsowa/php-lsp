===request===
document_highlight
===file===
<?php
function foo() {
    $x$0 = 1;
//  ^^ write
    $x .= '!';
//  ^^ write
    echo $x;
//       ^^ read
}
