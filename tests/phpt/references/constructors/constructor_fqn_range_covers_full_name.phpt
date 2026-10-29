===description===
Regression: constructor references via FQN (`new \App\Widget()`) produced a
range covering only `short_name.len()` characters from the `\` in `\App\Widget`,
i.e. `\App\W` instead of the full `\App\Widget`.
===request===
references include_declaration
===file:Widget.php===
<?php
namespace App;
class Widget {
    public function __con$0struct() {}
    //              ^^^^^^^^^^^ def
}

===file:main.php===
<?php
$w = new \App\Widget();
//       ^^^^^^^^^^^ ref
