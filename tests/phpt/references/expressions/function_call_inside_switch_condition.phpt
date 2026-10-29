===allow-invalid-php===
===request===
references include_declaration
===file===
<?php
function cla$0ssify(): string { return ''; }
//       ^^^^^^^^ def
switch (classify()) { default: break; }
//      ^^^^^^^^ ref
