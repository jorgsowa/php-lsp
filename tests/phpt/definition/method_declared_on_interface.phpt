===request===
definition
===file===
<?php
interface Runner { public function run(): void; }
//                                 ^^^ def
abstract class Base implements Runner {}
function go(Base $b) { $b->r$0un(); }
