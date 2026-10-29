===description===
Calling a genuinely missing method on a trait-using class still fires UndefinedMethod.
===file===
<?php
trait HasHello {
    public function hello(): void {}
}
class Foo {
    use HasHello;
}
function test(Foo $f): void {
    $f->reallyMissing();
//  ^^^^^^^^^^^^^^^^^^^ error: reallyMissing
}
