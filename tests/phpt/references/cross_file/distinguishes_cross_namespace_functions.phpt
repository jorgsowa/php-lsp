===description===
Two functions `greet` in different namespaces. Refs on `App\greet` must
NOT pick up the call to `Domain\greet`.
===request===
references include_declaration
===file:src/app.php===
<?php
namespace App;
function gr$0eet(): void {}
//       ^^^^^ def
  greet();
//^^^^^ ref

===file:src/domain.php===
<?php
namespace Domain;
function greet(): void {}
greet();
