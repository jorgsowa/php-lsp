===allow-invalid-php===
===request===
references include_declaration
===file===
<?php
class Ro$0ute {}
//    ^^^^^ def
#[Route]
//^^^^^ ref
class HomeController {}
