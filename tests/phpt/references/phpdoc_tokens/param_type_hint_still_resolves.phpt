===description===
The type half of `@param Type $var` is a real class name and must keep
resolving — same contrast the keyword tests draw between a bare keyword
and `self::`/`static::` right next to it.
===allow-invalid-php===
===request===
references include_declaration
===file===
<?php
class Widget {}
//    ^^^^^^ def
class Factory {
    /**
     * @param Widg$0et $w
     */
    public function make(Widget $w): void {}
//                       ^^^^^^ ref
}
