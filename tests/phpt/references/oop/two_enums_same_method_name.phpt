===description===
Two enums that both define `label()`. Refs on `Status::label` must NOT
pick up `Color::label` declarations and must use the span within the
correct enum member (not the first occurrence of the name in the file).
===request===
references include_declaration
===file===
<?php
enum Status {
    case Active;
    public function la$0bel(): string { return 'active'; }
    //              ^^^^^ def
}
enum Color {
    case Red;
    public function label(): string { return 'red'; }
}
echo Status::Active->label();
//                   ^^^^^ ref
