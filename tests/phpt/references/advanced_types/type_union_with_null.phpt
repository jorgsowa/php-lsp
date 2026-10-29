===request===
references include_declaration
===file===
<?php
class Tok$0en {
//    ^^^^^ def
}

function authenticate(): Token|null {
//                       ^^^^^ ref
    if (random_int(0, 1)) {
        return new Token();
        //         ^^^^^ ref
    }
    return null;
}

function validate(Token|null $token): bool {
//                ^^^^^ ref
    return $token !== null;
}
