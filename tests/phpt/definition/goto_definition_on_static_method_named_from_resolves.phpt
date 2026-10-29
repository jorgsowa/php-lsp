===description===
`from` is classified as a keyword token by the underlying lexer crate
(it's part of `yield from`), but PHP only reserves it directly after
`yield` — everywhere else, most commonly a backed enum's static factory
(`Suit::from('H')`), it's an ordinary method name. `is_php_keyword` must
not treat it as always-reserved, or this call site would incorrectly
resolve to nothing.
===request===
definition
===file===
<?php
class Suit {
    public static function from(string $value): self { return new self(); }
    //                     ^^^^ def
}
Suit::fr$0om('H');
