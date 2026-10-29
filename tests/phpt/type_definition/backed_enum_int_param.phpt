===description===
Backed enum (int-backed) with method parameter.
===request===
type_definition
===file===
<?php
class Logger {}
//    ^^^^^^ type
enum Priority: int {
    case HIGH = 1;
    case LOW = 0;
    public function log(Logger $logger$0): void {}
}
