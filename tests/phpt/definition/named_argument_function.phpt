===description===
Definition on a named argument label jumps to the matching parameter of the called function.
===request===
definition
===file===
<?php
function make(int $width, int $radius) {}
//                            ^^^^^^^ def
make(width: 1, rad$0ius: 2);
