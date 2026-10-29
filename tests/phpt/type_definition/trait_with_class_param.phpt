===description===
Type hints in trait methods should resolve correctly.
===request===
type_definition
===file===
<?php
class Config {}
//    ^^^^^^ type
trait Settings {
    public function load(Config $cfg$0): void {}
}
