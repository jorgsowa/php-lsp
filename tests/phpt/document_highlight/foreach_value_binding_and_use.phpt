===request===
document_highlight
===file===
<?php
function foo($arr) {
    foreach ($arr as $v$0alue) {
//                   ^^^^^^ write
        echo $value;
//           ^^^^^^ read
    }
}
