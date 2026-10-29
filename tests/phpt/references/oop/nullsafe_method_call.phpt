===description===
`$obj?->method()` must be found as a reference alongside `$obj->method()`.
===request===
references include_declaration
===file===
<?php
class Mailer {
    public function se$0nd(): void {}
    //              ^^^^ def
}
$m = new Mailer();
$m->send();
//  ^^^^ ref
$m?->send();
//   ^^^^ ref
