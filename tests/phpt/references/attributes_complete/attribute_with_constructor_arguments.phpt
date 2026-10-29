===request===
references include_declaration
===file===
<?php
class Required$0Arg {}
//    ^^^^^^^^^^^ def

class Form {
    #[RequiredArg('email', 'string')]
    //^^^^^^^^^^^ ref
    public string $email;
}
