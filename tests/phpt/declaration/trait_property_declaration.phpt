===request===
declaration
===file===
<?php
trait Timestampable {
    protected string $created$0;
    //                ^^^^^^^ decl
}
class Post {
    use Timestampable;
}
