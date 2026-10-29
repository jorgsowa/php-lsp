===config===
{"phpVersion": "8.5"}
===file===
<?php
$first = array_first([1]);
$last = array_last([1]);
$found = array_find([1], fn ($n) => true);
$any = array_any([1], fn ($n) => true);
$all = array_all([1], fn ($n) => true);
