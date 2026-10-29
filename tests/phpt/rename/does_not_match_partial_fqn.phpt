===description===
Edge case: FQN doesn't match due to partial name overlap.
When renaming App\Services\Foo, should NOT match App\Services\FooExtra.
This verifies the rename operation correctly enforces name boundaries.
===allow-invalid-php===
===request===
rename Bar
===file===
<?php
use App\Services\FooExtra;
use App\Services\Foo;
//               ^^^ rename
$obj = new Foo$0();
//         ^^^ rename
