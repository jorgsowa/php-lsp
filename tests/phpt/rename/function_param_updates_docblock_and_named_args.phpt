===description===
Renaming a function parameter also renames its `@param` tag and the
`name:` named arguments passed to it, but not same-named arguments of
another function.
===allow-invalid-php===
===request===
rename new
===file:a.php===
<?php
/**
 * @param int $old
//            ^^^^ rename
 */
function foo(int $ol$0d): int { return $old; }
//               ^^^^ rename
//                                   ^^^^ rename
function bar(int $old): void {}

===file:b.php===
<?php
foo(old: 1);
//  ^^^ rename
bar(old: 2);
