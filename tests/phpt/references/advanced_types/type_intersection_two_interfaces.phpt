===request===
references include_declaration
===file===
<?php
interface Printab$0le {
//        ^^^^^^^^^ def
}

interface Serializable {}

function export(Printable&Serializable $obj): string {
//              ^^^^^^^^^ ref
    return $obj->serialize();
}
