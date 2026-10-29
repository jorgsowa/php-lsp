===description===
Edge case: closure with multiple use() variables.
All variables in the use clause should be collected and renamed.
===allow-invalid-php===
===request===
rename $data
===file===
<?php
function process(): void {
    $input$0 = "data";
//  ^^^^^^ rename
    $output = "";
    $debug = false;
    $handler = function() use ($input, $output, $debug) {
    //                         ^^^^^^ rename
        if ($debug) {
            echo $input . $output;
            //   ^^^^^^ rename
        }
    };
    $handler();
}
