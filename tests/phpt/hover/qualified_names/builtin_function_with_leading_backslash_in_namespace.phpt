===description===
Hover on a backslash-qualified built-in function inside a namespace shows its signature.
===request===
hover
===file===
<?php
namespace App;
\json_en$0code([]);
===expect===
```php
function json_encode(mixed $value, int $flags = 0, int $depth = 512): string|false
```

---

(PHP 5 &gt;= 5.2.0, PECL json &gt;= 1.2.0)<br/> Returns the JSON representation of a value

[php.net documentation](https://www.php.net/function.json-encode)
