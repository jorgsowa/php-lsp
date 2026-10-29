===description===
`diagnostics.mixedUsage` is off by default — passing `mixed` to a typed
parameter produces no diagnostic unless opted in.
===file===
<?php
function takesString(string $s): void {}
function test(mixed $v): void {
    takesString($v);
}
