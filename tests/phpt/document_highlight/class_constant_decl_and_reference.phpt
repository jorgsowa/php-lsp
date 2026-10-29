===request===
document_highlight
===file===
<?php
class Foo {
    const BA$0R = 1;
    //    ^^^ ref
}
echo Foo::BAR;
//        ^^^ ref
