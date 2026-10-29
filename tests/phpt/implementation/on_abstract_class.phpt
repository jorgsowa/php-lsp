===request===
implementation
===file===
<?php
abstract class Base$0 { abstract public function handle(): void; }
class ConcreteA extends Base { public function handle(): void {} }
//    ^^^^^^^^^ impl
class ConcreteB extends Base { public function handle(): void {} }
//    ^^^^^^^^^ impl
