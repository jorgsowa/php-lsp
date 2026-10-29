===description===
Hover on `config('database')` shows the config file's `return` line.
===request===
hover
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
**config('database')**

```php
return [
```

`config/database.php`
