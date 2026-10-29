===description===
Hover on a fully-qualified call doesn't show a same-named function from another namespace.
===allow-invalid-php===
===request===
hover
===file===
<?php
namespace M;
function foo(string $s): string { return $s; }
\N\fo$0o(1);
===expect===
<no hover>
