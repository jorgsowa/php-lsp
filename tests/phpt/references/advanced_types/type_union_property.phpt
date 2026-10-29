===request===
references include_declaration
===file===
<?php
class Standa$0rd {
//    ^^^^^^^^ def
}

class Premium {}

class Account {
    public Standard|Premium $subscription;
    //     ^^^^^^^^ ref
}
