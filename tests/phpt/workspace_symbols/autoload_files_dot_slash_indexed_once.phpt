===description===
A `./`-prefixed autoload.files entry that the directory walk also reaches is indexed once.
===request===
workspace_symbols autoload_helper
===disk:composer.json===
{"autoload": {"files": ["./helpers.php", "helpers.php"]}}
===disk:helpers.php===
<?php
function autoload_helper() {}
===file===
<?php
autoload_helper();
===expect===
Function    autoload_helper @ helpers.php:1
