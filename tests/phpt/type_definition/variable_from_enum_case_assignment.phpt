===description===
Variable assigned from an enum case constant resolves to the enum type.
===request===
type_definition
===file===
<?php
enum Status { case Active; case Inactive; }
//   ^^^^^^ type
$s = Status::Active;
echo $s$0;
