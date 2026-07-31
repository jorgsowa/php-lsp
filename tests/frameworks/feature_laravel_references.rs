//! Protocol-wired regression tests for find-references against the real
//! Laravel framework corpus (~1 600 PHP files).
//!
//! These tests run the full LSP wire protocol (workspace scan → indexReady →
//! textDocument/references) against the cloned fixture so any regression in
//! cross-file reference discovery is caught before it ships.
//!
//! # Setup
//!
//! ```bash
//! scripts/setup_laravel_fixture.sh
//! ```
//!
//! # Running
//!
//! ```bash
//! cargo test --test frameworks laravel_references -- --ignored --nocapture
//! ```

use super::*;

use expect_test::expect;

/// Path to the cloned Laravel fixture.
const LARAVEL_SRC: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/benches/fixtures/laravel/src");

fn laravel_available() -> bool {
    std::path::Path::new(LARAVEL_SRC)
        .join("Illuminate/Support/Str.php")
        .exists()
}

// ── Str::lower ────────────────────────────────────────────────────────────────

/// `Illuminate\Support\Str::lower` is a static method called from at least
/// 8 files in the Laravel framework. The test verifies:
///
/// 1. The references request succeeds (no LSP error).
/// 2. At least 8 call sites are returned — a regression to 0 would indicate
///    the candidate prefilter or scoped ingestion broke cross-file resolution.
/// 3. `QueriesRelationships.php` (a known caller) appears in the result set.
#[ignore]
#[serial_test::serial(laravel)]
#[tokio::test]
async fn laravel_references_str_lower() {
    if !laravel_available() {
        println!("SKIP: Laravel fixture not found at {LARAVEL_SRC}");
        println!("      Run scripts/setup_laravel_fixture.sh to enable.");
        return;
    }

    let mut server = TestServer::with_root(LARAVEL_SRC).await;
    server.wait_for_index_ready_secs(60).await;

    let str_src = std::fs::read_to_string(
        std::path::Path::new(LARAVEL_SRC).join("Illuminate/Support/Str.php"),
    )
    .expect("Str.php not readable");

    // Open the file so the handler can read the word under the cursor.
    server.open("Illuminate/Support/Str.php", &str_src).await;

    // Line 755 (0-based) = line 756 in file: `    public static function lower($value)`
    // Character 27 = start of "lower".
    let resp = server
        .references("Illuminate/Support/Str.php", 755, 27, false)
        .await;

    let out = render_locations(&resp, &server.uri(""));
    assert!(
        out.matches('\n').count() + 1 >= 8,
        "expected >=8 references to Str::lower: {out}"
    );
    assert!(
        out.contains("QueriesRelationships"),
        "QueriesRelationships.php (known Str::lower caller) missing from results: {out}"
    );
    expect![[r#"
        Illuminate/Database/Eloquent/Concerns/QueriesRelationships.php:869:47-869:52
        Illuminate/Foundation/Console/DocsCommand.php:239:25-239:30
        Illuminate/Foundation/Console/DocsCommand.php:241:58-241:63
        Illuminate/Foundation/Console/DocsCommand.php:241:78-241:83
        Illuminate/Foundation/Console/DocsCommand.php:247:61-247:66
        Illuminate/Foundation/Console/ViewMakeCommand.php:184:29-184:34
        Illuminate/Support/Str.php:1568:87-1568:92
        Illuminate/Support/Str.php:1594:29-1594:34
        Illuminate/Support/Str.php:1719:41-1719:46
        Illuminate/Support/Str.php:1860:23-1860:28
        Illuminate/Support/Stringable.php:486:31-486:36
        Illuminate/Validation/Concerns/ValidatesAttributes.php:1466:41-1466:46
        Illuminate/Validation/Concerns/ValidatesAttributes.php:2492:24-2492:29"#]]
    .assert_eq(&out);
}

// ── Str class itself ──────────────────────────────────────────────────────────

/// References to the `Str` class appear in many files across the framework.
/// This test guards against the candidate prefilter accidentally excluding
/// files that only reference the class by a `use` import (no text occurrence
/// of the bare name "Str").
#[ignore]
#[serial_test::serial(laravel)]
#[tokio::test]
async fn laravel_references_str_class() {
    if !laravel_available() {
        println!("SKIP: Laravel fixture not found at {LARAVEL_SRC}");
        return;
    }

    let mut server = TestServer::with_root(LARAVEL_SRC).await;
    server.wait_for_index_ready_secs(60).await;

    let str_src = std::fs::read_to_string(
        std::path::Path::new(LARAVEL_SRC).join("Illuminate/Support/Str.php"),
    )
    .expect("Str.php not readable");

    server.open("Illuminate/Support/Str.php", &str_src).await;

    // Line 22 (0-based) = `class Str` — cursor on the class name (character 6).
    let resp = server
        .references("Illuminate/Support/Str.php", 22, 6, false)
        .await;

    let out = render_locations(&resp, &server.uri(""));
    assert!(
        out.matches('\n').count() + 1 >= 20,
        "expected >=20 references to class Str, refs truncated or broken: {out}"
    );
    // Spot-check a few known callers by name rather than snapshotting the
    // full multi-hundred-line result set across the whole framework corpus.
    for known_caller in [
        "Illuminate/Auth/Access/Gate.php",
        "Illuminate/Collections/Arr.php",
        "Illuminate/View/View.php",
    ] {
        assert!(
            out.contains(known_caller),
            "expected {known_caller} (known Str caller) in results: {out}"
        );
    }
}
