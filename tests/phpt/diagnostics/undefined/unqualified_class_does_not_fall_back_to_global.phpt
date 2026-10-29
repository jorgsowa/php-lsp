===description===
Contrast: a global-namespace *class* referenced unqualified from inside a
different namespace, with no `use` import, must still be flagged
`UndefinedClass` — classes do not fall back to the global namespace.
===file===
<?php
namespace {
    class GlobalOnlyThing {}
}

namespace App {
    function make(): void {
        $x = new GlobalOnlyThing();
    //           ^^^^^^^^^^^^^^^ error: GlobalOnlyThing
    }
}
