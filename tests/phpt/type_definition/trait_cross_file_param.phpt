===description===
Trait with cross-file type hint.
===request===
type_definition
===file:src/db.php===
<?php
class Connection {}
//    ^^^^^^^^^^ type

===file:src/main.php===
<?php
trait Database {
    public function query(Connection $conn$0): void {}
}
