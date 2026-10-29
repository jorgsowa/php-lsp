===request===
references include_declaration
===file===
<?php
class Cache {
    public static array $ite$0ms = [];
    //                   ^^^^^ def

    public static function add(string $key): void {
        self::$items[$key] = true;
        //     ^^^^^ ref
    }

    public static function get(string $key): mixed {
        return self::$items[$key] ?? null;
        //            ^^^^^ ref
    }

    public static function clear(): void {
        self::$items = [];
        //     ^^^^^ ref
    }
}
