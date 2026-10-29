===description===
Spreading a string-keyed array as a call's sole argument (`f(...$args)`)
binds each entry to the parameter of the same name — valid PHP 8.1+
named-argument syntax (confirmed: `php -l` passes and it runs, printing
"BobBob"). mir-analyzer's `expand_sole_spread_arg` now recognizes
string-keyed spreads and binds by parameter name instead of falling back
to a merged-union check against the first parameter.
===file===
<?php
function greet(string $name, int $times = 1): string {
    return str_repeat($name, $times);
}
function test(): string {
    $args = ['name' => 'Bob', 'times' => 2];
    return greet(...$args);
}
