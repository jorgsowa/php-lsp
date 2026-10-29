===request===
declaration
===file:Logger.php===
<?php
interface Logger {
    public function log(string $msg): void;
    //              ^^^ decl
}

===file:FileLogger.php===
<?php
class FileLogger implements Logger {
    public function log$0(string $msg): void {}
}
