===request===
implementation
===file===
<?php
interface Writ$0able { public function write(): void; }
class A implements Writable { public function write(): void {} }
//    ^ impl
class B implements Writable { public function write(): void {} }
//    ^ impl
