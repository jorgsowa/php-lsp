===description===
kind=Function must not return the interface method declaration.
===allow-invalid-php===
===request===
references include_declaration
===file===
<?php
function a$0dd(): void {}
//       ^^^ def
  add();
//^^^ ref
interface I {
    public function add(): void;
}
