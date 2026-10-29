===description===
Reproducer: a project polyfill that conditionally redefines a built-in.
If `ingest_stub_slice` is last-write-wins and the project file's parsed
`function restore_error_handler` overrides mir's stub, the call site may
still resolve — but the polyfill body is what ends up authoritative. This
test asserts that the call is *not* flagged undefined when a user-land
polyfill exists in the workspace.
===file:src/polyfill.php===
<?php
if (!function_exists('restore_error_handler')) {
    function restore_error_handler(): bool { return true; }
}

===file:src/main.php===
<?php
function _wrap(): void {
    restore_error_handler();
}
