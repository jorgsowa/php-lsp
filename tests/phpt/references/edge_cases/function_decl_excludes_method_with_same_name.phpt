===description===
Symmetric to references_on_method_decl_returns_method_refs_not_function_refs:
cursor on free-function declaration — method decl and method call must be excluded.
===allow-invalid-php===
===request===
references include_declaration
===file===
<?php
function a$0dd(): void {}
//       ^^^ def
  add();
//^^^ ref
class C { public function add(): void {} }
$c->add();
