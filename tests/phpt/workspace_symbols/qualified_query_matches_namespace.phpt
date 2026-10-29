===description===
A namespace-qualified query matches declarations in that namespace.
===request===
workspace_symbols N\foo
===file===
<?php
namespace N;
function foo() {}
===file:other.php===
<?php
namespace M;
function foo() {}
===expect===
Function    foo @ main.php:2 in N
