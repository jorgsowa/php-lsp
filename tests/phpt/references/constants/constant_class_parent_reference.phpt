===request===
references include_declaration
===file===
<?php
class Base {
    const VERS$0ION = '1.0';
    //    ^^^^^^^ def
}

class Extended extends Base {
    public function getVersion(): string {
        return parent::VERSION;
        //             ^^^^^^^ ref
    }
}

echo Extended::VERSION;
//             ^^^^^^^ ref
