===config===
{"phpVersion": "8.4"}
===file===
<?php
$all = array_all([1, 2, 3], fn ($n) => $n > 0);
