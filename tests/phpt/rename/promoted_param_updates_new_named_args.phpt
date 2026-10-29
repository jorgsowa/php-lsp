===description===
Renaming a promoted constructor parameter renames the property and the
`name:` arguments of `new`, from the parameter or from `$this->name`.
===allow-invalid-php===
===request===
rename new
===file:a.php===
<?php
class P {
    public function __construct(private int $o$0ld) {}
    //                                       ^^^ rename
    public function get(): int { return $this->old; }
    //                                         ^^^ rename
}

===file:b.php===
<?php
$p = new P(old: 1);
//         ^^^ rename
