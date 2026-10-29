===config===
{"phpVersion": "8.4"}
===file===
<?php
$found = array_find([1, 2, 3], fn ($n) => $n > 1);
