===config===
{"phpVersion": "8.4"}
===file===
<?php
$any = array_any([1, 2, 3], fn ($n) => $n > 1);
