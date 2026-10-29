===file===
<?php
class Inner { public function value(): int { return 1; } }
class Outer { public function inner(): ?Inner { return null; } }
$outer = null;
$outer?->inner()?->value();
