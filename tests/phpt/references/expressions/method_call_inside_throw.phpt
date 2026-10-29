===allow-invalid-php===
===request===
references include_declaration
===file===
<?php
class Factory {
    public function cre$0ate(): \Exception { return new \Exception(); }
    //              ^^^^^^ def
}
throw $factory->create();
//              ^^^^^^ ref
