===description===
A closure declared outside any class, using `$this`, later rebound to an
object via `Closure::bind()`/`bindTo()`/`call()` is valid PHP.
===file===
<?php
class Container {
    private int $value = 42;
}
$getter = function (): int {
    return $this->value;
};
$bound = Closure::bind($getter, new Container(), Container::class);
echo $bound();
