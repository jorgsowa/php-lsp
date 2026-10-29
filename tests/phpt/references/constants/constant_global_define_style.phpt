===request===
references include_declaration
===file===
<?php
define('APP_VE$0RSION', '2.0.0');
//      ^^^^^^^^^^^ def

echo APP_VERSION;
//   ^^^^^^^^^^^ ref

if (defined('APP_VERSION')) {
    echo APP_VERSION;
    //   ^^^^^^^^^^^ ref
}
