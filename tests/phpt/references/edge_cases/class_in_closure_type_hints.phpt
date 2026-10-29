===description===
Class names in closure parameter and return type hints must appear in
find-references results — mir v0.38 added ClassReference tracking for
closure/arrow-function type positions; the AST walker covers them too.
===allow-invalid-php===
===request===
references include_declaration
===file===
<?php
class Pay$0load {}
//    ^^^^^^^ def
$handler = function(Payload $p): Payload { return $p; };
//                  ^^^^^^^ ref
//                               ^^^^^^^ ref
$mapper = fn(Payload $x): Payload => $x;
//           ^^^^^^^ ref
//                        ^^^^^^^ ref
