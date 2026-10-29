===description===
Goto-definition on an enum method reference must jump to the method declaration.
===request===
definition
===file===
<?php
enum Color {
    case Red;
    public function label(): string { return ''; }
    //              ^^^^^ def
}
Color::Red->labe$0l();
