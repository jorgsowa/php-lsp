===description===
Class name appearing in a string literal before its declaration must not confuse
the implementation search; the result must point to the actual class declaration.
===request===
implementation
===file===
<?php
interface Logg$0able {}
$msg = 'Loggable is implemented by Logger right here';
class Logger implements Loggable {}
//    ^^^^^^ impl
