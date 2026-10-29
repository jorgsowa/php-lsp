===request===
references include_declaration
===file===
<?php
class Serialize$0Attribute {}
//    ^^^^^^^^^^^^^^^^^^ def

class Document {
    #[SerializeAttribute]
    //^^^^^^^^^^^^^^^^^^ ref
    public string $content;
}
