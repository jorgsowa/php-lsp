===description===
mixed is a pseudo-type that doesn't need references
===request===
references include_declaration
===file===
<?php
class Process$0or {
//    ^^^^^^^^^ def
}

function handle(mixed $data): void {
    if ($data instanceof Processor) {
        //               ^^^^^^^^^ ref
        $data->process();
    }
}
