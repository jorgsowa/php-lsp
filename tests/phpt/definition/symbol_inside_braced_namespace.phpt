===description===
Goto-definition on a symbol inside a braced namespace must find it.
===request===
definition
===file===
<?php
namespace App {
    function boot() {}
    //       ^^^^ def
    boo$0t();
}
