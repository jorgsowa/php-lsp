===request===
references include_declaration
===file===
<?php
class Bas$0eModel {
//    ^^^^^^^^^ def
}

interface Timestamped {}

function save(BaseModel&Timestamped $obj): void {
//            ^^^^^^^^^ ref
    $obj->save();
}
