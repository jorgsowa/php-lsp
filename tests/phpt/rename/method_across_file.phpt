===allow-invalid-php===
===request===
rename salute
===file===
<?php
class Greeter {
    public function he$0llo(): string { return 'hi'; }
    //              ^^^^^ rename
}
$g = new Greeter();
$g->hello();
//  ^^^^^ rename
