===description===
A class that composes a trait is excluded from narrowing (trait bodies
could resolve elsewhere), so the search keeps full scope. Either way the
result must be complete: the in-class call to the private method is found.
===request===
references include_declaration
===file===
<?php
trait Loggable {
    public function log(): void {}
}
class Service {
    use Loggable;
    private function pre$0pare(): void {}
    //               ^^^^^^^ def
    public function handle(): void {
        $this->prepare();
        //     ^^^^^^^ ref
    }
}
