===description===
Regression: the fallback declaration-range construction in handle_references
used `word.len() as u32` (UTF-8 bytes) instead of `encode_utf16().count()`
(UTF-16 code units). For a method with a non-ASCII name like `gérer` the
two differ, producing an end-column that points past the identifier.
The test verifies `includeDeclaration=true` returns exactly the expected
def+ref pair without an error — a wrong range would make the def location
land outside the actual identifier span.
===allow-invalid-php===
===request===
references include_declaration
===file===
<?php
class Doc {
    public function gér$0er(): void {}
    //              ^^^^^ def
}
$d = new Doc();
$d->gérer();
//  ^^^^^ ref
