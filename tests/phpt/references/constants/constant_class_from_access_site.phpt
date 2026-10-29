===description===
Cursor on a class constant ACCESS site must produce the same results as
cursor on the declaration — declaration + all usages.
===request===
references include_declaration
===file===
<?php
class Flags {
    const VERBOSE = 1;
    //    ^^^^^^^ def
    const DEBUG = 2;
}

$mode = Flags::VER$0BOSE;
//             ^^^^^^^ ref
if ($mode & Flags::VERBOSE) {}
//                 ^^^^^^^ ref
