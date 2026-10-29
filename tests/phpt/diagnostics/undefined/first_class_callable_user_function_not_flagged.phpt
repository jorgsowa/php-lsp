===description===
Same as above but for a project-defined function, proving the first-class
callable syntax doesn't only work for stub/built-in functions.
===file===
<?php
function greet(string $name): string { return "hi $name"; }
function _wrap(): void {
    $fn = greet(...);
}
