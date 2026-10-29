===description===
Cursor on the interface method declaration: must find both the
implementing class's method declaration and call sites.
===request===
references include_declaration
===file===
<?php
interface Renderable {
    public function ren$0der(): string;
    //              ^^^^^^ def
}
class Page implements Renderable {
    public function render(): string { return ''; }
    //              ^^^^^^ def
}
$page = new Page();
echo $page->render();
//          ^^^^^^ ref
