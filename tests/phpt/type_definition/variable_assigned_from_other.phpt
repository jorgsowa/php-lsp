===description===
Variable assigned from another variable's type is now tracked.
===request===
type_definition
===file===
<?php
class Result {}
//    ^^^^^^ type
$value = new Result();
$copy = $value;
$copy$0->process();
