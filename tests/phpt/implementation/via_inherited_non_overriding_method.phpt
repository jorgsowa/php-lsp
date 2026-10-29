===description===
An implementor that satisfies an interface method purely by *inheriting*
it from an ancestor which itself does not declare `implements` — the
ancestor never overrides the method locally.
===request===
implementation
===file===
<?php
interface Indexable$0 {
    public function serialize(): array;
}

abstract class DocumentBlock {
    public function serialize(): array { return []; }
}

final class DocumentBlockText extends DocumentBlock implements Indexable {}
//          ^^^^^^^^^^^^^^^^^ impl

final class DocumentBlockImage extends DocumentBlock implements Indexable {}
//          ^^^^^^^^^^^^^^^^^^ impl
