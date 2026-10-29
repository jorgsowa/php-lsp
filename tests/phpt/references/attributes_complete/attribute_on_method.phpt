===request===
references include_declaration
===file===
<?php
class Route$0Attribute {}
//    ^^^^^^^^^^^^^^ def

class Controller {
    #[RouteAttribute('/users', 'GET')]
    //^^^^^^^^^^^^^^ ref
    public function getUsers(): array {
        return [];
    }
}
