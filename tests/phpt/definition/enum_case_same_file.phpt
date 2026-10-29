===description===
Goto-definition on an enum case reference must jump to the case declaration.
===request===
definition
===file===
<?php
enum Suit {
    case Hearts;
    //   ^^^^^^ def
}
$x = Suit::Hearts$0;
