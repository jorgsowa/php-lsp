===description===
Hover on a built-in function marks its by-reference parameters with `&`.
===request===
hover
===file===
<?php
str_re$0place('a', 'b', 'abc');
===expect===
```php
function str_replace(string|array<int|string, string> $search, string|array<int|string, scalar|Stringable|null> $replace, string|array<int|string, string> $subject, int &$count = null): ($subject is array ? array<int, string> : string)
```

---

Replace all occurrences of the search string with the replacement string

[php.net documentation](https://www.php.net/function.str-replace)
