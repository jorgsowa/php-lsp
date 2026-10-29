===file===
<?php
namespace LspTest;
class Broken {
    public function f(): void {
        nonexistent_fn();
//      ^^^^^^^^^^^^^^^^ error: nonexistent_fn
    }
}
