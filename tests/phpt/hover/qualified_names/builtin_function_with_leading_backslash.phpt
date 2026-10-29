===description===
Hover on a built-in function written with a leading backslash shows its signature.
===request===
hover
===file===
<?php
\str$0len('b');
===expect===
```php
function strlen(string $string): int<0, max>
```

---

Get string length

[php.net documentation](https://www.php.net/function.strlen)
