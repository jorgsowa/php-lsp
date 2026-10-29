===request===
references include_declaration
===file===
<?php
class Config {
    public static string $app$0_name = '';
    //                    ^^^^^^^^ def
    public static string $version = '1.0';

    public static function init(): void {
        self::$app_name = 'MyApp';
        //     ^^^^^^^^ ref
        self::$version = self::$app_name . ' v' . self::$version;
        //                      ^^^^^^^^ ref
    }
}
