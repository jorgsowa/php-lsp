===description===
`self` keyword in class parameter resolves to the containing class.
===request===
type_definition
===file===
<?php
class User {
//    ^^^^ type
    public function duplicate(self $other$0): self {}
}
