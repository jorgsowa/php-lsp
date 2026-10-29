===request===
references include_declaration
===file===
<?php
class Rea$0d {
//    ^^^^ def
}

class Write {}
class Delete {}

function f(Read|Write|Delete $x): void {}
//         ^^^^ ref
function g(): Read {}
//            ^^^^ ref
function h($x) { if ($x instanceof Read) {} }
//                                 ^^^^ ref
