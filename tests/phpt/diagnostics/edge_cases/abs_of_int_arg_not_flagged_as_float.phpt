===description===
`abs(int)` returns `int` when the argument is an `int` literal or parameter.
Regression guard for a past false-positive where the analyzer reported
`float|int` for the return type, flagging a type mismatch when the result
was passed to an `int` parameter (fixed upstream in mir; kept green here).
===file===
<?php
function takesInt(int $x): void {}
function test(int $n): void {
    takesInt(abs($n));
}
