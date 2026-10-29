===allow-invalid-php===
===request===
rename new
===file:a.php===
<?php
class P {
    public function __construct(private int $old) {}
    //                                       ^^^ rename
    public function get(): int { return $this->o$0ld; }
    //                                         ^^^ rename
}

===file:b.php===
<?php
$p = new P(old: 1);
//         ^^^ rename
