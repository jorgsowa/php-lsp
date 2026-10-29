===request===
references include_declaration
===file===
<?php
class Credi$0t {
//    ^^^^^^ def
}

function f(Credit|null $x): void {}
//         ^^^^^^ ref
function g(): Credit {}
//            ^^^^^^ ref
function h(): void { new Credit(); }
//                       ^^^^^^ ref
