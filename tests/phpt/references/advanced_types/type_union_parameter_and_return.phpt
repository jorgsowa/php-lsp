===request===
references include_declaration
===file===
<?php
class Succe$0ss {
//    ^^^^^^^ def
}

function f1(): Success|Error {}
//             ^^^^^^^ ref
function f2(Success|Error $x) {}
//          ^^^^^^^ ref
function f3(): Success {}
//             ^^^^^^^ ref
function f4(Success $x): void {}
//          ^^^^^^^ ref
function f5(): Success {}
//             ^^^^^^^ ref
function f6(): void { new Success(); }
//                        ^^^^^^^ ref
function f7($x): void { if ($x instanceof Success) {} }
//                                        ^^^^^^^ ref
