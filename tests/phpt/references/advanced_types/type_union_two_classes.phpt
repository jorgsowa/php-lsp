===request===
references include_declaration
===file===
<?php
class Request$0er {
//    ^^^^^^^^^ def
}

class Response {}

function handle(Requester|Response $item): void {
//              ^^^^^^^^^ ref
    if ($item instanceof Requester) {
    //                   ^^^^^^^^^ ref
    }
}
