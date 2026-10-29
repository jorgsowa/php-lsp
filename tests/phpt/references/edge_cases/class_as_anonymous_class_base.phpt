===allow-invalid-php===
===request===
references include_declaration
===file===
<?php
class Ba$0se {}
//    ^^^^ def
$x = new class extends Base {};
//                     ^^^^ ref
