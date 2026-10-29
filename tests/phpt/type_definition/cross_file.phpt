===request===
type_definition
===file:a.php===
<?php
$obj = new Mailer();
$obj$0->send();

===file:mailer.php===
<?php
class Mailer {}
//    ^^^^^^ type
