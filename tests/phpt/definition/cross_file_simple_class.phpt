===description===
Cross-file goto-definition for a namespace-free class.
===request===
definition
===file:greeter.php===
<?php
class Greeter {}
//    ^^^^^^^ def

===file:user.php===
<?php
$g = new Gr$0eeter();
