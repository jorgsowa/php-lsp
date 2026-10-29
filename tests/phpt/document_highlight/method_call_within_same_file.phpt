===request===
document_highlight
===file===
<?php
class Greeter {
    public function hel$0lo(): void {}
    //              ^^^^^ ref
}
$g = new Greeter();
$g->hello();
//  ^^^^^ ref
$g->hello();
//  ^^^^^ ref
