===description===
Cursor on `parent::__construct()` in a child constructor must resolve to
the *parent* class's own instantiation sites. `parent::` is compile-time
resolved in PHP — it always names the literal `extends` class, never
subject to late static binding — so it must not resolve to `new Child(...)`,
which invokes a different (overriding) constructor.
===request===
references include_declaration
===file===
<?php
class Base {
    public function __construct(int $id) {}
}
class Child extends Base {
    public function __construct(int $id, string $name) {
        parent::__con$0struct($id);
        //      ^^^^^^^^^^^ ref
    }
}
new Child(1, 'Alice');
new Base(2);
//  ^^^^ ref
