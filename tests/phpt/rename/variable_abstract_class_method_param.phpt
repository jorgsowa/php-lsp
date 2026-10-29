===description===
Renaming a variable parameter in an abstract class method produces edits.
===allow-invalid-php===
===request===
rename $data
===file===
<?php
abstract class Processor {
    abstract public function process($in$0put): string;
    //                               ^^^^^^ rename
}
