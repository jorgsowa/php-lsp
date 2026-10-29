===description===
Includes built from `__DIR__ . '/path'` link to the path relative to the file's directory.
===allow-invalid-php===
===request===
document_links
===file:src/main.php===
<?php
include 'inc3.php';
include __DIR__ . '/inc.php';
require_once __DIR__ . '/x.php';
require(__DIR__ . '/../lib/y.php');
include dirname(__FILE__) . '/z.php';
include $prefix . '/dynamic.php';
===expect===
1:9-1:17 -> src/inc3.php
2:19-2:27 -> src/inc.php
3:24-3:30 -> src/x.php
4:19-4:32 -> lib/y.php
5:29-5:35 -> src/z.php
