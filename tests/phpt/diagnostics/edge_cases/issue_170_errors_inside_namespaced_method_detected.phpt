===file===
<?php
namespace LspTest;

class Broken
{
    public int $count = 0;

    public function bump(): int
    {
        $this->count++;
        return $this->count;
    }

    public function obviouslyBroken(): int
    {
        nonexistent_function();
//      ^^^^^^^^^^^^^^^^^^^^^^ error: nonexistent_function
        $x = new UnknownClass();
//               ^^^^^^^^^^^^ error: UnknownClass
        return 0;
    }
}
