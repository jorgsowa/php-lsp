===description===
`@see`'s target is a real class name and must keep resolving — only the
`@template` parameter name is documentation-only, not every word inside
every tag.
===allow-invalid-php===
===request===
references include_declaration
===file===
<?php
class Base {}
//    ^^^^ def
/**
 * @see Bas$0e
 */
class Box {}
