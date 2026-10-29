===description===
Nullable union type resolution now returns all matching types.
===request===
type_definition
===file===
<?php
class Success {}
//    ^^^^^^^ type
class Error {}
//    ^^^^^ type
function handle(Success|Error $result$0): void {}
