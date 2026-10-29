===request===
references include_declaration
===file===
<?php
class Depreca$0ted {}
//    ^^^^^^^^^^ def

#[Deprecated]
//^^^^^^^^^^ ref
function oldFunction(): void {
    echo 'This is deprecated';
}
