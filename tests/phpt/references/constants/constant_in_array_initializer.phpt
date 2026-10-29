===request===
references include_declaration
===file===
<?php
class HttpCode {
    const O$0K = 200;
    //    ^^ def
    const NOT_FOUND = 404;
}

$responses = [
    HttpCode::OK => 'success',
    //        ^^ ref
    HttpCode::NOT_FOUND => 'not found',
];

function status(): int {
    return HttpCode::OK;
    //               ^^ ref
}
