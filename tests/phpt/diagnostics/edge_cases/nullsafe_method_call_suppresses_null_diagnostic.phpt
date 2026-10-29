===description===
The nullsafe operator explicitly guards against null and must suppress the
null-dereference diagnostic a plain `->` on the same value would trigger.
===file===
<?php
class Box { public function value(): int { return 1; } }
$box = null;
$box?->value();
