===description===
Type definition on a parameter inside an enum method resolves correctly.
===request===
type_definition
===file===
<?php
class Logger {}
//    ^^^^^^ type
enum Status {
    case Active;
    public function log(Logger $l$0): void {}
}
