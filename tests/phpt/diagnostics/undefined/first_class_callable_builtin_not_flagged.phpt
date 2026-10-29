===description===
`strlen(...)` (PHP 8.1 first-class callable syntax) referencing a known
built-in function must not be flagged `UndefinedFunction` — a naive
resolver keyed on "function name immediately followed by `(`" could treat
the `...` placeholder as an unusual argument list and mis-handle the call.
===file===
<?php
function _wrap(): void {
    $fn = strlen(...);
}
