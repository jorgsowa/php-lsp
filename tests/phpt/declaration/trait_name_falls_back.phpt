===request===
declaration
===file===
<?php
trait Loggable$0 {
//    ^^^^^^^^ decl
    public function log(): void {}
}
class Page {
    use Loggable;
}
