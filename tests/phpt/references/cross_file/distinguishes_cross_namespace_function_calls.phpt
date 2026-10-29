===description===
Cross-namespace function filter: refs on `Alpha\process` must not include
calls to `Beta\process` in another file.
===request===
references include_declaration
===file:alpha.php===
<?php
namespace Alpha;
function pro$0cess(): void {}
//       ^^^^^^^ def
  process();
//^^^^^^^ ref

===file:beta.php===
<?php
namespace Beta;
function process(): void {}
process();
