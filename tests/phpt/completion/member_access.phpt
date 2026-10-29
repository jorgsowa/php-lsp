===description===
Member completion lists public methods and properties.
===allow-invalid-php===
===request===
completion
===file===
<?php
class Box {
    public int $size = 0;
    public function open(): void {}
    private function hidden(): void {}
}
$b = new Box();
$b->$0
===expect===
Property    $size
Method      open
