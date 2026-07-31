//! Comprehensive hover coverage.

use super::*;

use expect_test::expect;

#[tokio::test]
async fn hover_class_identifier() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class Gre$0eter {}
"#,
        expect![[r#"
            ```php
            class Greeter
            ```"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_enum_identifier() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
enum Stat$0us { case Active; case Inactive; }
"#,
        expect![[r#"
            ```php
            enum Status
            ```"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_function() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php function gr$0eet(): void {}"#,
        expect![[r#"
            ```php
            function greet(): void
            ```"#]],
    )
    .await;
}

/// Template parameters are shown as-is in the signature (T, not resolved).
#[tokio::test]
async fn hover_function_with_template_shows_template_in_docblock() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
/**
 * @template T
 * @param T $value
 * @return T
 */
function identi$0ty($value) { return $value; }
"#,
        expect![[r#"
            ```php
            function identity($value)
            ```

            ---

            **@return** `T`
            **@param** `T` `$value`
            **@template** `T`"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_function_with_throws_shows_tag() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
/**
 * @throws \RuntimeException When the operation fails
 */
function ri$0sky(): void {}
"#,
        expect![[r#"
            ```php
            function risky(): void
            ```

            ---

            **@throws** `\RuntimeException` — When the operation fails"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_interface_identifier() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
interface Writ$0able { public function write(): void; }
"#,
        expect![[r#"
            ```php
            interface Writable
            ```"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_method() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class Greeter {
    public function he$0llo(): string { return 'hi'; }
}"#,
        expect![[r#"
            ```php
            public function hello(): string
            ```"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_method_after_instanceof_narrows_type() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class Greeter { public function hello() {} }
function process(mixed $x) {
    if ($x instanceof Greeter) {
        $x->hel$0lo();
    }
}
"#,
        expect![[r#"
            ```php
            Greeter::hello()
            ```"#]],
    )
    .await;
}

/// instanceof narrowing with @param type hint.
#[tokio::test]
async fn hover_method_after_instanceof_with_param_type() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class Service {
    public function execute() {}
}
function handle(object $obj) {
    if ($obj instanceof Service) {
        $obj->exec$0ute();
    }
}
"#,
        expect![[r#"
            ```php
            Service::execute()
            ```"#]],
    )
    .await;
}

/// Two unrelated classes declare a same-named method; hovering a call on a
/// receiver with a known concrete type must resolve to that receiver's own
/// class, not whichever declaration comes first.
#[tokio::test]
async fn hover_method_call_resolves_receiver_class() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class Mailer { public function process(string $to): bool {} }
class Queue  { public function process(int $id): void {} }
$mailer = new Mailer();
$mailer->pro$0cess('');
"#,
        expect![[r#"
            ```php
            Mailer::process(string $to): bool
            ```"#]],
    )
    .await;
}

/// PHP method dispatch is case-insensitive, so a call site spelled in a
/// different case than the declaration must still resolve — matches the
/// dispatch semantics `references.rs` already applies to method lookups.
#[tokio::test]
async fn hover_method_call_is_case_insensitive() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class Mailer { public function process(string $to): bool {} }
$mailer = new Mailer();
$mailer->PROC$0ESS('');
"#,
        expect![[r#"
            ```php
            Mailer::process(string $to): bool
            ```"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_method_without_instanceof_does_not_narrow() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class Processor { public function process(): int {} }
class Uploader { public function process(): string {} }
function test(mixed $obj) {
    // No instanceof narrowing here — $obj could be either class. With two
    // conflicting `process()` candidates in scope, hover falls back to the
    // first-declared one rather than reporting neither/ambiguous.
    $obj->proce$0ss();
}
"#,
        expect![[r#"
            ```php
            public function process(): int
            ```"#]],
    )
    .await;
}

// ── @template / PHPDoc generic resolution ────────────────────────────────────

/// Hovering on a function declaration with @template shows the @template in docblock.
#[tokio::test]
async fn hover_missing_symbol_returns_nothing() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(r#"<?php fo$0o();"#, expect!["<no hover>"])
        .await;
}

#[tokio::test]
async fn hover_on_empty_file_returns_null_not_error() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.open("empty.php", "").await;
    let resp = s.hover("empty.php", 0, 0).await;
    assert!(
        resp["error"].is_null(),
        "hover errored on empty file: {resp:?}"
    );
    assert!(
        resp["result"].is_null(),
        "hover on empty file should be null, got: {:?}",
        resp["result"]
    );
}

#[tokio::test]
async fn hover_past_eof_does_not_crash() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.open("short.php", "<?php\nfunction f(): void {}\n").await;
    let resp = s.hover("short.php", 500, 500).await;
    assert!(resp["error"].is_null(), "hover past EOF errored: {resp:?}");
    assert!(
        resp["result"].is_null(),
        "hover past EOF should have null result, got: {resp:?}"
    );
}

// ── Backed enum ───────────────────────────────────────────────────────────────

/// `enum Status: string` must include `: string` in the hover so the caller
/// knows the backing type.
#[tokio::test]
async fn hover_static_method() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class Registry {
    public static function ge$0t(string $k): mixed {}
}"#,
        expect![[r#"
            ```php
            public static function get(string $k): mixed
            ```"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_static_method_in_match_not_broken() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class Checker {
    public static function isValid($x) { return true; }
}
match ($x) {
    1 => Checker::isVal$0id($x),
}
"#,
        expect![[r#"
            ```php
            Checker::isValid($x)
            ```"#]],
    )
    .await;
}

// ── instanceof narrowing integration tests ──────────────────────────────────

// ── clone-with (PHP 8.5) ────────────────────────────────────────────────────

#[tokio::test]
async fn hover_clone_with_result_inherits_type() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class Point { public int $x; public int $y; }
$p = new Point();
$q = clone($p, ['x' => 1]);
echo $q$0->x;
"#,
        expect!["`$q` `Point`"],
    )
    .await;
}

#[tokio::test]
async fn hover_no_result_without_known_source_type() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
$q = clone($unknown, ['x' => 1]);
echo $q$0->x;
"#,
        expect!["<no hover>"],
    )
    .await;
}

/// Hover's response always includes a `range` covering the hovered word
/// (`word_range_at`) — no test in the suite checks it, since `render_hover`
/// only surfaces markdown `contents`, never `range`. A regression in
/// `word_range_at`'s boundary math (off-by-one column, wrong word split)
/// would pass every other hover test unnoticed.
#[tokio::test]
async fn hover_range_covers_the_hovered_word() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let opened = s
        .open_fixture(
            r#"<?php
class Greeter {}
$g = new Gree$0ter();
"#,
        )
        .await;
    let c = opened.cursor().clone();
    let resp = s.hover(&c.path, c.line, c.character).await;
    let r = &resp["result"]["range"];
    let rendered = format!(
        "{}:{}-{}:{}",
        r["start"]["line"], r["start"]["character"], r["end"]["line"], r["end"]["character"]
    );
    expect!["2:9-2:16"].assert_eq(&rendered);
}

/// After `if ($x instanceof Foo)`, hovering on `$x->method()` inside the block
/// should resolve to Foo::method().
#[tokio::test]
async fn hover_variable_is_scoped_to_method() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class Widget {}
class Invoice {}
class Service {
    public function a(): void { $result = new Widget(); }
    public function b(): void { $res$0ult = new Invoice(); }
}
"#,
        expect!["`$result` `Invoice`"],
    )
    .await;
}
