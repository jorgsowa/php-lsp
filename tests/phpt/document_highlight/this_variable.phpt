===request===
document_highlight
===file===
<?php
class Foo {
    public function bar() {
        $th$0is->baz();
//      ^^^^^ read
        $this->qux();
//      ^^^^^ read
    }
}
