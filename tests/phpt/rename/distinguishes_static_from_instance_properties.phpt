===description===
Renaming must respect static properties and not confuse them with instance properties.
===allow-invalid-php===
===request===
rename total
===file===
<?php
class Config {
    public static $instance;
    public $count;
    //      ^^^^^ rename
    public function test(): void {
        $this->coun$0t++;
        //     ^^^^^ rename
    }
}
