===request===
references include_declaration
===file===
<?php
class Config {
    const DEBU$0G = true;
    //    ^^^^^ def

    public static function isDebug(): bool {
        return self::DEBUG;
        //           ^^^^^ ref
    }

    public function check(): void {
        echo self::DEBUG ? 'debug' : 'prod';
        //         ^^^^^ ref
    }
}
