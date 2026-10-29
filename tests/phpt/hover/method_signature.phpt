===description===
Hover on a method call shows the declared signature.
===request===
hover
===file===
<?php
class Greeter {
    public function hello(string $name): string { return "hi $name"; }
}
(new Greeter())->hel$0lo('x');
===expect===
```php
Greeter::hello(string $name): string
```
