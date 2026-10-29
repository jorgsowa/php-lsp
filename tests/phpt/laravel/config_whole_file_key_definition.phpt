===description===
`config('database')` names a whole config file and resolves to its `return`.
===request===
definition
===disk:artisan===
#!/usr/bin/env php
===disk:config/database.php===
<?php
return [
    'default' => 'mysql',
];
===file:app.php===
<?php
$db = config('data$0base');
===expect===
config/database.php:1:0-1:6
