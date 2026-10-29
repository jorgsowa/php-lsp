===request===
implementation
===file===
<?php
interface Work$0er { public function execute(): void; }
class JobA implements Worker { public function execute(): void {} }
//    ^^^^ impl
class JobB implements Worker { public function execute(): void {} }
//    ^^^^ impl
class JobC implements Worker { public function execute(): void {} }
//    ^^^^ impl
