===description===
A free-function call inside an enum method body must be found by references.
===allow-invalid-php===
===request===
references include_declaration
===file===
<?php
function hel$0per(): void {}
//       ^^^^^^ def
enum Status {
    public function label(): string { return helper(); }
    //                                       ^^^^^^ ref
}
