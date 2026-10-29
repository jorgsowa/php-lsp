===description===
Duplicate interface declaration in the same file should produce an error.
===file===
<?php
interface Logger {}
  interface Logger {}
//^^^^^^^^^^^^^^^^^^^ error: Interface Logger has already been defined
