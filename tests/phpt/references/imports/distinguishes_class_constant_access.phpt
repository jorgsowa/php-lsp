===request===
references include_declaration
===file===
<?php
class Status {
    const ACTIVE = 1;
    //    ^^^^^^ def
}
$x = Status::AC$0TIVE;
//           ^^^^^^ ref
