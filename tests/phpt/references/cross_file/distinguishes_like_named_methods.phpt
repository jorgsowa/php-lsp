===description===
Two classes both define `process()`. Refs on Mailer::process must NOT
pick up Queue::process calls.
===request===
references include_declaration
===file===
<?php
class Mailer {
    public function pro$0cess(): void {}
    //              ^^^^^^^ def
}
class Queue {
    public function process(): void {}
}
$m = new Mailer();
$m->process();
//  ^^^^^^^ ref
$q = new Queue();
$q->process();
