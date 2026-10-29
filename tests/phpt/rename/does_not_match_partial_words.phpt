===description===
Rename must match exact word boundaries and not rename partial matches.
===allow-invalid-php===
===request===
rename baz
===file===
<?php
function foo$0() {}
//       ^^^ rename
function foobar() {}
function barfoo() {}
  foo();
//^^^ rename
foobar();
barfoo();
