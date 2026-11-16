===request===
references
===file:Shell.php===
<?php
namespace App;

class Shell {
    const VERS$0ION = '1.0';
}

===file:info.php===
<?php
namespace App;

function info($shell) {
    return $shell::VERSION;
    //             ^^^^^^^ ref
}

===file:Reloader.php===
<?php
namespace App;

class Reloader {
    const VERSION = 1;
}

function reloaderVersion(Reloader $reloader) {
    return $reloader::VERSION;
}
