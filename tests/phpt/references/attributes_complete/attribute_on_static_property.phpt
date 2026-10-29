===request===
references include_declaration
===file===
<?php
class Singleton$0Marker {}
//    ^^^^^^^^^^^^^^^ def

class Cache {
    #[SingletonMarker]
    //^^^^^^^^^^^^^^^ ref
    public static Cache $instance;

    public static function getInstance(): self {
        return self::$instance;
    }
}
