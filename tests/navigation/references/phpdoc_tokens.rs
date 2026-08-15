//! Regression guard: PHPDoc documentation-only tokens must never resolve as
//! a referenceable symbol — the docblock analogue of `keyword_tokens.rs`.
//!
//! Before the fix (ROADMAP item 0a), a cursor on any of these fell through
//! to the same unguarded bareword workspace search real keywords used to:
//! the tag name itself (`param` in `@param`, including arbitrary/custom
//! tags no hardcoded list would ever cover), a `@template` parameter name,
//! or the `$varName` half of `@param`/`@var`/`@property*`/`@method` bodies.
//! Each collides here with a real, unrelated same-named declaration
//! elsewhere in the workspace to prove the gate — not just an absence of
//! any candidate to resolve to.

use super::*;
use expect_test::expect;

/// `@template T` collision: an unrelated top-level function literally named
/// `T` sits elsewhere in the workspace. Before the fix, the bare `T` in the
/// template tag resolved to it via the same ungated bareword fallback the
/// keyword tests above guard against.
#[tokio::test]
async fn template_parameter_name_ignores_cross_file_name_collision() {
    let mut s = TestServer::new().await;
    let out = s
        .check_references(
            r#"//- /src/Unrelated.php
<?php
function T(): void {}

//- /src/Box.php
<?php
/**
 * @template T$0 of object
 */
class Box {}
"#,
        )
        .await;
    expect!["<none>"].assert_eq(&out);
}

/// `@see`'s target is a real class name and must keep resolving — only the
/// `@template` parameter name is documentation-only, not every word inside
/// every tag.
#[tokio::test]
async fn see_tag_target_still_resolves() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_references_annotated(
        r#"<?php
class Base {}
//    ^^^^ def
/**
 * @see Bas$0e
 */
class Box {}
"#,
    )
    .await;
}

/// The upper-bound *type* half of `@template T of Base` should, in
/// principle, keep resolving the same way `@see`'s target does above — only
/// the parameter name `T` is meant to be documentation-only.
///
/// Separate, pre-existing bug found while building this gate (unrelated to
/// it: reproduces identically whether or not `is_unresolvable_docblock_token_at`
/// gates anything here, since `Base` — the bound, not the parameter name —
/// is never gated by it). `Base`'s reference resolves to a bogus location
/// anchored at the docblock's own `/**` instead of at `Base`'s declaration.
/// Isolated to `@template` specifically: swapping the tag for `@see` (above)
/// or plain prose resolves correctly. Most likely mir's per-file analysis
/// recognizes `@template ... of X` as a generics annotation and conflates
/// any offset inside that docblock with the annotated declaration's own
/// (buggy) span, the same class of span-conflation bug the keyword fix
/// (`f4bd5c57`) fixed for modifier tokens — but for a tag mir itself
/// interprets, not a bareword fallback this crate controls. Needs its own
/// investigation into mir's per-file analysis; out of scope for the
/// PHPDoc-annotation bareword gate this file otherwise covers.
#[tokio::test]
async fn template_bound_type_still_resolves() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_references_annotated(
        r#"<?php
class Base {}
//    ^^^^ def
/**
 * @template T of Bas$0e
 */
class Box {}
"#,
    )
    .await;
}

/// `@param Type $varName` collision: an unrelated property literally named
/// `count` sits elsewhere in the workspace. The `$count` doc-token must
/// never resolve to it.
#[tokio::test]
async fn param_doc_variable_name_ignores_cross_file_name_collision() {
    let mut s = TestServer::new().await;
    let out = s
        .check_references(
            r#"//- /src/Unrelated.php
<?php
class Unrelated {
    public int $count = 0;
}

//- /src/Counter.php
<?php
class Counter {
    /**
     * @param int $count$0 starting value
     */
    public function __construct(int $count) {}
}
"#,
        )
        .await;
    expect!["<none>"].assert_eq(&out);
}

/// The type half of `@param Type $var` is a real class name and must keep
/// resolving — same contrast the keyword tests draw between a bare keyword
/// and `self::`/`static::` right next to it.
#[tokio::test]
async fn param_type_hint_still_resolves() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_references_annotated(
        r#"<?php
class Widget {}
//    ^^^^^^ def
class Factory {
    /**
     * @param Widg$0et $w
     */
    public function make(Widget $w): void {}
//                       ^^^^^^ ref
}
"#,
    )
    .await;
}

/// `@property-read`/`@method`/`@var` doc-token variable names get the same
/// treatment as `@param`'s — one representative collision per tag covers
/// the shared `is_unresolvable_body_token` code path.
#[tokio::test]
async fn property_and_method_doc_variable_names_ignore_cross_file_name_collision() {
    let mut s = TestServer::new().await;
    let out = s
        .check_references(
            r#"//- /src/Unrelated.php
<?php
class Unrelated {
    public string $name = '';
    public function make(): void {}
}

//- /src/User.php
<?php
/**
 * @property-read string $name$0
 */
class User {}
"#,
        )
        .await;
    expect!["<none>"].assert_eq(&out);
}

/// The tag name itself (`internal`, a custom/vendor-style tag no hardcoded
/// list would need to enumerate) must never resolve, even when a real,
/// identically-spelled function sits elsewhere in the workspace — proving
/// the gate is structural (position-based: immediately after `@` inside a
/// doc-block) rather than a list of known tag spellings.
#[tokio::test]
async fn custom_tag_name_never_resolves_without_a_hardcoded_list() {
    let mut s = TestServer::new().await;
    let out = s
        .check_references(
            r#"//- /src/Unrelated.php
<?php
function internal(): void {}

//- /src/Value.php
<?php
/**
 * @intern$0al
 */
final class Value {}
"#,
        )
        .await;
    expect!["<none>"].assert_eq(&out);
}

/// A plain `/* ... */` block comment (not a doc-block: no second `*`) must
/// not be treated as PHPDoc at all — `@word`-shaped text inside one still
/// goes through ordinary bareword resolution.
#[tokio::test]
async fn at_word_in_plain_block_comment_is_not_gated() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_references_annotated(
        r#"<?php
function param(): void {}
//       ^^^^^ def
/* @param int $x */
$x = par$0am();
//   ^^^^^ ref
"#,
    )
    .await;
}
