===request===
references include_declaration
===file===
<?php
class Counter {
    public static int $valu$0e = 0;
    //                 ^^^^^ def
}

function incrementCounter(): void {
    Counter::$value++;
    //        ^^^^^ ref
}

function resetCounter(): void {
    Counter::$value = 0;
    //        ^^^^^ ref
}

function getCounter(): int {
    return Counter::$value;
    //               ^^^^^ ref
}
