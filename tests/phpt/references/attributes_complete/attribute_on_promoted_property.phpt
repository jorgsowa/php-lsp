===request===
references include_declaration
===file===
<?php
class Immuta$0ble {}
//    ^^^^^^^^^ def

class User {
    public function __construct(
        #[Immutable]
        //^^^^^^^^^ ref
        public readonly string $id,
    ) {}
}
