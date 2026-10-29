===request===
references include_declaration
===file===
<?php
class Data$0base {
//    ^^^^^^^^ def
}

function f(): ?Database {}
//             ^^^^^^^^ ref
function g(): Database|null {}
//            ^^^^^^^^ ref
function h(): void { new Database(); }
//                       ^^^^^^^^ ref
