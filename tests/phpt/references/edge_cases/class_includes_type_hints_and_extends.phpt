===description===
When cursor is on a class name (not __construct), refs include structural
usages: type hints, `extends`, and `instanceof`. No `new Ev$0ent()` is
present so the codebase fast path (which only tracks instantiation sites)
falls back to the AST walker that catches all class references.
===allow-invalid-php===
===request===
references include_declaration
===file===
<?php
class Ev$0ent {}
//    ^^^^^ def
class UserEvent extends Event {}
//                      ^^^^^ ref
function dispatch(Event $e): void {}
//                ^^^^^ ref
$e = null;
if ($e instanceof Event) {}
//                ^^^^^ ref
