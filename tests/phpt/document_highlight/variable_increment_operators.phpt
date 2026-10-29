===request===
document_highlight
===file===
<?php
function foo() {
    $x$0++;
//  ^^ ref
    ++$x;
//    ^^ ref
    --$x;
//    ^^ ref
    $x--;
//  ^^ ref
}
