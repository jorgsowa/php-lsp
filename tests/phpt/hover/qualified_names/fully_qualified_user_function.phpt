===description===
Hover on a fully-qualified user function call shows its signature.
===request===
hover
===file===
<?php
namespace N;
function foo(int $x): int { return $x; }
\N\fo$0o(1);
===expect===
```php
function foo(int $x): int
```
