===file===
<?php
  class A extends B {}
  class B extends A {}
//^^^^^^^^^^^^^^^^^^^^ error: Class B has a circular inheritance chain
