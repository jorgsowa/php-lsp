===description===
$arg in outer scope must not bleed into the enum method's highlight set.
===request===
document_highlight
===file===
<?php
$arg = 0;
enum Status {
    public function label(
        $a$0rg
    //  ^^^^ write
    ) {
        return $arg + 1;
    //         ^^^^ read
    }
}
