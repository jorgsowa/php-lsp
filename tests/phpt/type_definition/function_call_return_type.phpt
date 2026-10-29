===description===
Function call return type is resolved via mir's recorded symbol type.
===request===
type_definition
===file===
<?php
class Document {}
//    ^^^^^^^^ type
function getDocument(): Document {
    return new Document();
}
$doc = getDocument();
$doc$0->render();
