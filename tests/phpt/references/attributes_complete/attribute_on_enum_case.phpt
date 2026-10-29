===request===
references include_declaration
===file===
<?php
class Default$0Case {}
//    ^^^^^^^^^^^ def

enum Priority {
    #[DefaultCase]
    //^^^^^^^^^^^ ref
    case Low;
    case Medium;
    case High;
}
