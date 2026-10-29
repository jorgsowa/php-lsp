===request===
document_highlight
===file===
<?php
function foo($arr) {
    foreach ($arr as $k$0ey => $value) {
//                   ^^^^ write
        echo $key;
//           ^^^^ read
    }
}
