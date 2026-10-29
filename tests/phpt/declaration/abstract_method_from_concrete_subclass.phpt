===request===
declaration
===file===
<?php
abstract class Base {
    abstract public function build(): void;
    //                       ^^^^^ decl
}
class Impl extends Base {
    public function build$0(): void {}
}
