//! Regression guard: PHP keyword/modifier tokens must never resolve as a
//! referenceable symbol.
//!
//! Before the fix, `symbol_kind_at` fell through to `SymbolKind::Function`
//! for any lowercase word not preceded by `->`/`::`, so clicking on a bare
//! keyword like `final`/`readonly`/`class` made mir search the *entire*
//! workspace for anything sharing that literal name — surfacing unrelated
//! symbols (e.g. a `$final` property elsewhere) and paying the cost of a
//! full, un-narrowed candidate scan on every such click. These tests drive
//! the real `textDocument/references` request and assert both the result
//! (empty) and the read-path cost (no extra parses, no reference-index
//! locks), the same guards `stress.rs` uses for the narrowed-scope paths.

use super::*;
use expect_test::expect;

/// Line/utf-16-col of the first occurrence of `needle` in `text`.
fn pos_of(text: &str, needle: &str) -> (u32, u32) {
    for (line, content) in text.lines().enumerate() {
        if let Some(byte_col) = content.find(needle) {
            let col = content[..byte_col].encode_utf16().count() as u32;
            return (line as u32, col);
        }
    }
    panic!("`{needle}` not found in fixture text");
}

const TARGET_PHP: &str = r#"<?php

namespace App;

use App\Contracts\Greets;

abstract class Target implements Greets
{
    final public const int LIMIT = 1;

    private static ?self $instance = null;

    public readonly int $value;

    public function __construct(private readonly int $x)
    {
        $this->value = $x;
    }

    abstract protected function compute(): int;

    final public static function make(): static
    {
        return new static(0);
    }
}
"#;

/// One token per category the old fallback misclassified as a free function:
/// declaration keywords, class modifiers, visibility, and pseudo-type words.
const KEYWORDS: &[&str] = &[
    "namespace",
    "use",
    "abstract",
    "class",
    "implements",
    "final",
    "const",
    "private",
    "static",
    "self",
    "readonly",
    "function",
    "protected",
    "new",
];

#[tokio::test]
async fn keyword_tokens_have_no_references_and_touch_no_candidate_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("target.php"), TARGET_PHP).unwrap();

    // A same-named `$<keyword>` property per token: a bare-name fallback
    // would surface these (sigil-only names are the one place a keyword
    // spelling is a legal PHP identifier) — this is the exact shape of the
    // reported `$final` property collision.
    let unrelated_props: String = KEYWORDS
        .iter()
        .map(|k| format!("    public int ${k} = 0;\n"))
        .collect();
    std::fs::write(
        dir.path().join("unrelated.php"),
        format!("<?php\nclass Unrelated\n{{\n{unrelated_props}}}\n"),
    )
    .unwrap();

    // Noise files: each legitimately contains every keyword above as real
    // PHP syntax, so a bare-name fallback has a full workspace of candidate
    // files to fan out into.
    for i in 0..15 {
        std::fs::write(
            dir.path().join(format!("noise_{i}.php")),
            format!(
                "<?php\nnamespace App;\nuse App\\Contracts\\Greets;\nabstract class Noise{i} implements Greets\n{{\n    final public const int LIMIT = 1;\n    private static ?self $instance = null;\n    abstract protected function compute(): int;\n    final public static function make(): static {{ return new static(); }}\n}}\n"
            ),
        )
        .unwrap();
    }

    let mut s = TestServer::with_root(dir.path()).await;
    s.wait_for_index_ready().await;
    // Drain the post-index warm-analysis sweep first: it keeps taking
    // RefIndex locks in the background after indexReady, which would
    // otherwise land inside the before/after window below and be mistaken
    // for locks taken by the keyword query itself.
    assert!(
        s.wait_for_warm_sweeps(1).await,
        "post-index warm sweep did not complete"
    );
    s.open("target.php", TARGET_PHP).await;

    for keyword in KEYWORDS {
        let (line, col) = pos_of(TARGET_PHP, keyword);

        let parses_before = s.debug_stats_parses().await;
        let locks_before = s.debug_stats_ref_index_locks().await;
        let resp = s.references("target.php", line, col, true).await;
        let parses_after = s.debug_stats_parses().await;
        let locks_after = s.debug_stats_ref_index_locks().await;

        assert!(
            resp["error"].is_null(),
            "references(`{keyword}`) errored: {resp:?}"
        );
        assert_eq!(
            render_locations(&resp, &s.uri("")),
            "<none>",
            "keyword `{keyword}` must never resolve as a referenceable symbol \
             (would otherwise leak the same-named `${keyword}` property in unrelated.php)"
        );
        assert_eq!(
            parses_after,
            parses_before,
            "references(`{keyword}`) parsed {} extra candidate doc(s); a keyword \
             token must short-circuit before any workspace scan",
            parses_after - parses_before
        );
        assert_eq!(
            locks_after,
            locks_before,
            "references(`{keyword}`) took {} reference-index lock(s); a keyword \
             token must never reach the candidate-file / index lookup path",
            locks_after - locks_before
        );
    }
}

/// The keyword bail-out must not shadow the legitimate `self::`/`static::`
/// resolution path it sits right next to in `symbol_kind_at`: `self` and
/// `static` are themselves reserved words, but immediately before `::` they
/// must still resolve through to the method they qualify.
#[tokio::test]
async fn scope_resolution_keywords_still_resolve_through_double_colon() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_references_annotated(
        r#"<?php
class C {
    public static function ma$0ke(): static { return new static(); }
    //                     ^^^^ def
    public static function via(): static { return self::make(); }
    //                                                  ^^^^ ref
}
"#,
    )
    .await;
}

/// Magic constants (`__CLASS__`, `__FUNCTION__`, ...) are reserved words too
/// — they satisfy `word_at_position`'s identifier character class just like
/// any keyword, and a lowercase-first check alone doesn't exclude them
/// (they start with `_`, not an uppercase letter), so they hit the exact
/// same free-function fallback `final`/`abstract`/`class` used to.
#[tokio::test]
async fn magic_constant_has_no_references() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_references(
            r#"<?php
class Widget {
    public function name(): string { return __CLAS$0S__; }
}
"#,
        )
        .await;
    expect!["<none>"].assert_eq(&out);
}

/// `__halt_compiler` is a hard reserved keyword present on php.net's
/// reserved.keywords.php but was missing from this project's keyword list
/// entirely until it was backfilled by sourcing from `php_lexer::resolve_keyword`
/// — pin it the same way every other reserved word above is pinned.
#[tokio::test]
async fn halt_compiler_has_no_references() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_references(
            r#"<?php
__halt_$0compiler();
"#,
        )
        .await;
    expect!["<none>"].assert_eq(&out);
}

/// `from` is classified as a keyword token by `php_lexer::resolve_keyword`
/// (it's part of `yield from`), but PHP only reserves it directly after
/// `yield` — everywhere else, most commonly a backed enum's static factory
/// (`Suit::from('H')`), it's an ordinary method name. This is the inverse of
/// every other test in this file: the bare-keyword gate must *not* fire here,
/// or a naive reuse of `resolve_keyword` would silently break references on
/// any symbol literally named `from`.
#[tokio::test]
async fn method_named_from_still_resolves_references() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_references_annotated(
        r#"<?php
class Suit {
    public static function from(string $value): self { return new self(); }
    //                     ^^^^ def
}
Suit::fr$0om('H');
//    ^^^^ ref
"#,
    )
    .await;
}
