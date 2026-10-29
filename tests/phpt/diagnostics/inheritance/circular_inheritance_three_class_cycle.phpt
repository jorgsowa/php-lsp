===file===
<?php
  class A extends B {}
  class B extends C {}
  class C extends A {}
//^^^^^^^^^^^^^^^^^^^^ error: Class C has a circular inheritance chain
