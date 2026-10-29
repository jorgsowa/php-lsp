===description===
Cross-file goto-definition for a free function (not a class).
===request===
definition
===file:helpers.php===
<?php
function helperFn() {}
//       ^^^^^^^^ def

===file:main.php===
<?php
helperFn$0();
