===description===
Backed enum (string-backed) typed as parameter.
===request===
type_definition
===file===
<?php
enum Status: string {
//   ^^^^^^ type
    case ACTIVE = 'active';
    case INACTIVE = 'inactive';
}
function process(Status $status$0): void {}
