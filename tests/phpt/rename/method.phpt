===description===
Renaming a method updates its declaration and call site.
===request===
rename greet
===file===
<?php
class Greeter {
    public function hello(): void {}
}
(new Greeter())->hel$0lo();
===expect===
// main.php
2:20-2:25 → "greet"
4:17-4:22 → "greet"
