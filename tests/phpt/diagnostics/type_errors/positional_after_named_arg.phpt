===description===
Only the parser's syntax error is reported, not an `InvalidNamedArgument`
for the positional argument.
===allow-invalid-php===
===file===
<?php
function bar(int $a, int $b): void {}
bar(a: 1, 2);
//        ^ error: cannot use positional argument after named argument
