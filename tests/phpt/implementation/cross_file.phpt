===request===
implementation
===file:src/Contract.php===
<?php
interface Operat$0or { public function run(): void; }

===file:src/impl/Add.php===
<?php
class Add implements Operator { public function run(): void {} }
//    ^^^ impl

===file:src/impl/Sub.php===
<?php
class Sub implements Operator { public function run(): void {} }
//    ^^^ impl
