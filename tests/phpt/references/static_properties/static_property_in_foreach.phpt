===request===
references include_declaration
===file===
<?php
class Logger {
    public static array $message$0s = [];
    //                   ^^^^^^^^ def
}

function logErrors(): void {
    foreach (Logger::$messages as $msg) {
    //                ^^^^^^^^ ref
        echo $msg;
    }

    Logger::$messages = [];
    //       ^^^^^^^^ ref
}
