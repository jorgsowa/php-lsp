===description===
Renaming a variable parameter in an interface method (bodyless) produces edits.
===allow-invalid-php===
===request===
rename $msg
===file===
<?php
interface Logger {
    public function log($mes$0sage): void;
    //                  ^^^^^^^^ rename
}
