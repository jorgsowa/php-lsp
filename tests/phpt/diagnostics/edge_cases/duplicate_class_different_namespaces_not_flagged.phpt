===description===
Classes with the same short name in different namespaces must NOT be flagged.
===file===
<?php
namespace AppA;
class Foo {}

namespace AppB;
class Foo {}
