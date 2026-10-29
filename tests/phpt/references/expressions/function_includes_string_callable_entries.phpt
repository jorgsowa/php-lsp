===description===
Bare string callables are function references in PHP APIs such as
`array_map`, `usort`, and event/hook registration. A complete references
implementation should include those string literals when resolving from the
target function declaration.
===request===
references include_declaration
===file===
<?php
function norm$0alize(string $s): string { return trim($s); }
//       ^^^^^^^^^ def
$items = array_map('normalize', [' a ']);
//                  ^^^^^^^^^ ref
register_shutdown_function('normalize');
//                          ^^^^^^^^^ ref
