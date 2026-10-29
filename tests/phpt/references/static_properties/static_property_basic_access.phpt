===request===
references include_declaration
===file===
<?php
class Registry {
    public static string $instan$0ce = '';
    //                    ^^^^^^^^ def
}

$r = Registry::$instance;
//              ^^^^^^^^ ref

Registry::$instance = 'value';
//         ^^^^^^^^ ref
