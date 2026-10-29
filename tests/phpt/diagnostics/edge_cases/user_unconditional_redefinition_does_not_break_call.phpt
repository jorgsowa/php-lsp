===description===
Reproducer: an unconditional user-land redefinition of a built-in.
PHP would refuse this at runtime, but the LSP still parses it; if the
stub-ingest path is last-write-wins, the project's body silently replaces
mir's stub. The call site should still resolve.
===allow-invalid-php===
===file:src/redef.php===
<?php
function restore_error_handler(): bool { return true; }

===file:src/main.php===
<?php
function _wrap(): void {
    restore_error_handler();
}
