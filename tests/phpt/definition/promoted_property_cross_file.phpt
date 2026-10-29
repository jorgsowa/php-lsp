===description===
True cross-file definition: cursor in one file, promoted param declaration
in a different file's constructor.
===request===
definition
===file:src/Repository.php===
<?php
class Repository {
    public function __construct(private object $conn) {}
    //                                          ^^^^ def
}

===file:src/main.php===
<?php
$r = new Repository($db);
$r->co$0nn;
