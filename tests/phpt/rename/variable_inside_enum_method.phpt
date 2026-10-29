===description===
Renaming a variable inside an enum method produces edits for all occurrences.
===allow-invalid-php===
===request===
rename value
===file===
<?php
enum Status {
    public function label($a$0rg) { return $arg + 1; }
    //                    ^^^^ rename
    //                                   ^^^^ rename
}
