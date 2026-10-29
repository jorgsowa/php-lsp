===description===
Calling an undefined function is reported at the call name.
===file===
<?php
$x = nonexistent_function();
//   ^^^^^^^^^^^^^^^^^^^^^^ error: Function nonexistent_function() is not defined
