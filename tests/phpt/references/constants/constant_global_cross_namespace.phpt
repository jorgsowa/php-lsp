===request===
references include_declaration
===file:config.php===
<?php
namespace Config;

const DB_H$0OST = 'localhost';
//    ^^^^^^^ def

===file:database.php===
<?php
namespace App\Database;

function connect() {
    $host = \Config\DB_HOST;
    //              ^^^^^^^ ref
}

class Connection {
    private string $host = \Config\DB_HOST;
    //                             ^^^^^^^ ref
}
