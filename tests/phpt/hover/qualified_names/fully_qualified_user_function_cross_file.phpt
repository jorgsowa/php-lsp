===description===
Hover on a fully-qualified call to a function declared in another file shows its signature.
===request===
hover
===file:src/functions.php===
<?php
namespace N;
function foo(int $x): int { return $x; }
===file:main.php===
<?php
\N\fo$0o(1);
===expect===
```php
function foo(int $x): int
```
