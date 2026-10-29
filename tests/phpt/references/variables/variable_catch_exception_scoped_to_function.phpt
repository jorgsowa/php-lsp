===description===
Catch-block exception variable body usage must be found and must not bleed
into the same-named variable in another function's catch block.
Note: the catch binding itself (`catch (Exception $e)`) is not collected
as a "declaration" by the scope walker — only body usages are returned.
===allow-invalid-php===
===request===
references include_declaration
===file===
<?php
function run(): void {
    try {
        doStuff();
    } catch (Exception $e$0) {
        log($e->getMessage());
        //  ^^ ref
    }
}
function other(): void {
    try { doOther(); } catch (Exception $e) { log($e); }
}
