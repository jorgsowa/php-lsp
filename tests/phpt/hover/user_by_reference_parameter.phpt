===description===
Hover on a user function places `&` between a by-reference parameter's type and name.
===request===
hover
===file===
<?php
function byref(array &$a, &...$rest): void {}
by$0ref($x);
===expect===
```php
function byref(array &$a, &...$rest): void
```
