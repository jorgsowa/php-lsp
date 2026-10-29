===description===
References to a function include its declaration when asked.
===request===
references include_declaration
===file===
<?php
function greet(): void {}
gr$0eet();
greet();
===expect===
main.php:1:9-1:14
main.php:2:0-2:5
main.php:3:0-3:5
