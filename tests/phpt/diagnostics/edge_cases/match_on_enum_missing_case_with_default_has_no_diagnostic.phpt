===description===
Contrast: a non-exhaustive `match` with a `default` arm covers every
remaining case at runtime and must not be flagged.
===file===
<?php
enum Suit {
    case Hearts;
    case Spades;
}
function label(Suit $s): string {
    return match ($s) {
        Suit::Hearts => 'hearts',
        default => 'other',
    };
}
