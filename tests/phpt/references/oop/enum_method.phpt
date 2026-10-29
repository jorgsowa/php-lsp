===request===
references include_declaration
===file===
<?php
enum Status {
    case Active;
    public function lab$0el(): string { return 'active'; }
    //              ^^^^^ def
}
echo Status::Active->label();
//                   ^^^^^ ref
echo Status::Active->label();
//                   ^^^^^ ref
