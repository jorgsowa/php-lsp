===request===
declaration
===file:Base.php===
<?php
abstract class Base {
    abstract public function build(): void;
    //                       ^^^^^ decl
}

===file:Impl.php===
<?php
class Impl extends Base {
    public function build$0(): void {}
}
