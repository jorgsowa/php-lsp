===description===
Renaming a property via a `->access` site must update the declaration and
all other access sites. The cursor must be on the bare name after `->`,
not on the `$prop` declaration (which is treated as a variable rename).
===allow-invalid-php===
===request===
rename total
===file===
<?php
class Counter {
    public int $count = 0;
    //          ^^^^^ rename
    public function inc(): void { $this->coun$0t++; }
    //                                   ^^^^^ rename
    public function get(): int  { return $this->count; }
    //                                          ^^^^^ rename
}
