===file===
<?php
class C {
    public function run(): void {
        nonexistent_fn();
//      ^^^^^^^^^^^^^^^^ error: nonexistent_fn
    }
}
