===description===
The upper-bound *type* half of `@template T of Base` should, in
principle, keep resolving the same way `@see`'s target does above — only
the parameter name `T` is meant to be documentation-only.

Separate, pre-existing bug found while building this gate (unrelated to
it: reproduces identically whether or not `is_unresolvable_docblock_token_at`
gates anything here, since `Base` — the bound, not the parameter name —
is never gated by it). `Base`'s reference resolves to a bogus location
anchored at the docblock's own `/**` instead of at `Base`'s declaration.
Isolated to `@template` specifically: swapping the tag for `@see` (above)
or plain prose resolves correctly. Most likely mir's per-file analysis
recognizes `@template ... of X` as a generics annotation and conflates
any offset inside that docblock with the annotated declaration's own
(buggy) span, the same class of span-conflation bug the keyword fix
(`f4bd5c57`) fixed for modifier tokens — but for a tag mir itself
interprets, not a bareword fallback this crate controls. Needs its own
investigation into mir's per-file analysis; out of scope for the
PHPDoc-annotation bareword gate this file otherwise covers.
===allow-invalid-php===
===request===
references include_declaration
===file===
<?php
class Base {}
//    ^^^^ def
/**
 * @template T of Bas$0e
 */
class Box {}
