===description===
Same coverage starting from a usage site instead of the declaration —
pins the already-working direction so a future regression in either
direction is caught.
===request===
references include_declaration
===file===
<?php
enum Status {
    case Active;
    //   ^^^^^^ def
}
echo Status::Act$0ive;
//           ^^^^^^ ref
