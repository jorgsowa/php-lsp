===description===
Duplicate trait declaration in the same file should produce an error.
===file===
<?php
trait Serializable {}
  trait Serializable {}
//^^^^^^^^^^^^^^^^^^^^^ error: Trait Serializable has already been defined
