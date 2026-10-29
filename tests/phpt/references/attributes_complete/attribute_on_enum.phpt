===request===
references include_declaration
===file===
<?php
class Backable$0Enum {}
//    ^^^^^^^^^^^^ def

#[BackableEnum]
//^^^^^^^^^^^^ ref
enum Status: int {
    case Active = 1;
    case Inactive = 0;
}
