===request===
references include_declaration
===file:Registry.php===
<?php
class ServiceRegistry {
    public static ?\Psr\Container $contai$0ner = null;
    //                             ^^^^^^^^^ def
}

===file:Service.php===
<?php
function getService(string $name): mixed {
    return ServiceRegistry::$container?->get($name);
    //                       ^^^^^^^^^ ref
}

class Bootstrap {
    public static function init(\Psr\Container $c): void {
        ServiceRegistry::$container = $c;
        //                ^^^^^^^^^ ref
    }
}
