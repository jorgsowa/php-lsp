===description===
Variable used inside a static method must be scoped to that method.
===request===
references include_declaration
===file===
<?php
class Factory {
    public static function make(string $type$0): object {
        //                             ^^^^^ def
        return match ($type) {
        //            ^^^^^ ref
            'foo' => new Foo(),
            default => new stdClass(),
        };
    }
    public static function list(string $type): array { return []; }
}
