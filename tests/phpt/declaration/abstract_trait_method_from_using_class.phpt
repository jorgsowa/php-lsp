===request===
declaration
===file===
<?php
trait Renderable {
    abstract public function render(): string;
    //                       ^^^^^^ decl
}
class Page {
    use Renderable;
    public function render$0(): string { return ''; }
}
