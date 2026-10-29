===request===
references include_declaration
===file===
<?php
class Async {}
class Cach$0ed {}
//    ^^^^^^ def

class Service {
    #[Async]
    #[Cached]
    //^^^^^^ ref
    public function fetchData(): array {
        return [];
    }
}
