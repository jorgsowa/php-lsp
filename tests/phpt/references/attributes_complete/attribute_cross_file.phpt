===request===
references include_declaration
===file:src/Attributes.php===
<?php
class Api$0Endpoint {}
//    ^^^^^^^^^^^ def

===file:src/Controllers/UserController.php===
<?php
class UserController {
    #[ApiEndpoint('/users', 'GET')]
    //^^^^^^^^^^^ ref
    public function list(): array {}

    #[ApiEndpoint('/users/{id}', 'GET')]
    //^^^^^^^^^^^ ref
    public function show(int $id): array {}
}
