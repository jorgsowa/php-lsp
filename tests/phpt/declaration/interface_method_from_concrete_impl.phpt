===request===
declaration
===file===
<?php
interface Logger {
    public function log(string $msg): void;
    //              ^^^ decl
}
class FileLogger implements Logger {
    public function log$0(string $msg): void {}
}
