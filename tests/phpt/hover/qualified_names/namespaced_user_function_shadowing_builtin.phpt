===description===
Hover on a namespaced user function named like a built-in shows no php.net link.
===request===
hover
===file===
<?php
namespace App;
function strlen(string $s): int { return 0; }
str$0len('x');
===expect===
```php
function strlen(string $s): int
```
