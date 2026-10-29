===description===
`diagnostics.missingTypes` is off by default — interface methods without
return/param annotations are not flagged unless opted in.
===file===
<?php
interface Logger {
    public function log($message);
}
