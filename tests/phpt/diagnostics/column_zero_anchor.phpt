===description===
`//<` anchors an annotation at column 0, where a statement-level diagnostic starts.
===file===
<?php
nonexistent_function();
//<^^^^^^^^^^^^^^^^^^^^^^ error: Function nonexistent_function() is not defined
