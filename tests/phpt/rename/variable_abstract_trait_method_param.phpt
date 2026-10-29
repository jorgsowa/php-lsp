===description===
Renaming a variable parameter in an abstract trait method produces edits.
===allow-invalid-php===
===request===
rename $input
===file===
<?php
trait Formattable {
    abstract public function format($da$0ta): string;
    //                              ^^^^^ rename
}
