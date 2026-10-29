===request===
references include_declaration
===file===
<?php
class Validate$0Attribute {}
//    ^^^^^^^^^^^^^^^^^ def

function process(
    #[ValidateAttribute('email')]
    //^^^^^^^^^^^^^^^^^ ref
    string $email
): void {
    // validate
}
