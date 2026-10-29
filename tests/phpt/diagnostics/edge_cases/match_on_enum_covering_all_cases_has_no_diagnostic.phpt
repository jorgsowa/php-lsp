===description===
A `match` covering every case of a pure enum, with no `default`, is
exhaustive and must not be flagged.
===file===
<?php
enum Suit {
    case Hearts;
    case Spades;
}
function label(Suit $s): string {
    return match ($s) {
        Suit::Hearts => 'hearts',
        Suit::Spades => 'spades',
    };
}
