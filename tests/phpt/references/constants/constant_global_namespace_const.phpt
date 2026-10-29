===request===
references include_declaration
===file===
<?php
namespace App;

const MAX_S$0IZE = 1000;
//    ^^^^^^^^ def

function validate($input) {
    if (strlen($input) > MAX_SIZE) {
    //                   ^^^^^^^^ ref
        throw new Exception('too large');
    }
}

class Validator {
    public function check(string $s): bool {
        return strlen($s) <= MAX_SIZE;
        //                   ^^^^^^^^ ref
    }
}
