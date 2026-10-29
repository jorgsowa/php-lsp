===request===
references include_declaration
===file:Config.php===
<?php
class AppConfig {
    const TIMEO$0UT = 30;
    //    ^^^^^^^ def
    const MAX_RETRIES = 5;
}

===file:Client.php===
<?php
$timeout = AppConfig::TIMEOUT;
//                    ^^^^^^^ ref

function retry() {
    for ($i = 0; $i < AppConfig::TIMEOUT; $i++) {
    //                           ^^^^^^^ ref
        try_request();
    }
}
