===description===
Hover on a `namespace\`-relative function call resolves against the current namespace.
===request===
hover
===file===
<?php
namespace N;
function bar(int $x): int { return $x; }
namespace\ba$0r(1);
===expect===
```php
function bar(int $x): int
```
