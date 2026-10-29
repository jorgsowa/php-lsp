===description===
The `class_exists()`-guarded conditional-declaration polyfill pattern
(common in Composer packages shimming a class only when missing) must not
break resolution for the class's own body or its call sites — the class
equivalent of `user_polyfill_does_not_break_builtin_restore_error_handler`.
===file:src/polyfill.php===
<?php
if (!class_exists(JsonExtra::class)) {
    interface JsonExtra {}
}

===file:src/main.php===
<?php
function _wrap(JsonExtra $x): void {
}
