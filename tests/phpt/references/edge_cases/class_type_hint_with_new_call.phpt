===description===
When a class appears both as a type hint AND in a new expression, find-references
must include ALL sites — not just the new call. This is the regression case where
the salsa fast path returned only `new Widget()` and silently dropped type hints.
===allow-invalid-php===
===request===
references include_declaration
===file===
<?php
class Wi$0dget {}
//    ^^^^^^ def
function foo(Widget $w): Widget {}
//           ^^^^^^ ref
//                       ^^^^^^ ref
$x = new Widget();
//       ^^^^^^ ref
