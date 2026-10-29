===request===
references include_declaration
===file===
<?php
trait Timestampable {
    public function touc$0hAt(): void {}
    //              ^^^^^^^ def
}
class Post {
    use Timestampable;
}
$p = new Post();
$p->touchAt();
//  ^^^^^^^ ref
$p->touchAt();
//  ^^^^^^^ ref
