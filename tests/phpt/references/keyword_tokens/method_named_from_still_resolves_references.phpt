===description===
`from` is classified as a keyword token by `php_lexer::resolve_keyword`
(it's part of `yield from`), but PHP only reserves it directly after
`yield` — everywhere else, most commonly a backed enum's static factory
(`Suit::from('H')`), it's an ordinary method name. This is the inverse of
every other test in this file: the bare-keyword gate must *not* fire here,
or a naive reuse of `resolve_keyword` would silently break references on
any symbol literally named `from`.
===allow-invalid-php===
===request===
references include_declaration
===file===
<?php
class Suit {
    public static function from(string $value): self { return new self(); }
    //                     ^^^^ def
}
Suit::fr$0om('H');
//    ^^^^ ref
