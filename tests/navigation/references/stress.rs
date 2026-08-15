//! Read-path cost guards for `textDocument/references`.
//!
//! The method reference path resolves usages from mir's per-file `analyze_file`
//! query and must never materialize a php-lsp `ParsedDoc` for the
//! text-matching candidate set — doing so reintroduced whole-workspace parsing
//! that grew with project size. These tests drive the real LSP request against
//! a background-scanned workspace and assert, via `$/php-lsp/debugStats`, that
//! the request parses (next to) nothing regardless of how many candidate files
//! mention the symbol's name.
//!
//! The `references_stress` fixture is one declaring class (`Target`) plus 30
//! unrelated classes that each textually contain `compute` and `process`, so
//! the text pre-filter admits every file as a candidate.

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

fn target_text() -> String {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/references_stress/src/Target.php");
    std::fs::read_to_string(path).expect("read Target.php fixture")
}

#[tokio::test]
async fn warm_sweep_completes_after_index_ready() {
    // The post-index analysis warm sweep must run to completion in the real
    // server so first references answer from warm memos. Regression guard for
    // the sweep silently never finishing (e.g. blocked on yields or cancelled).
    let mut s = TestServer::with_fixture("references_stress").await;
    s.wait_for_index_ready().await;
    assert!(
        s.wait_for_warm_sweeps(1).await,
        "post-index warm sweep did not complete"
    );
}

#[tokio::test]
async fn references_public_method_does_not_parse_candidate_files() {
    // `Target::process` is public, so the candidate set is NOT visibility-
    // narrowed — all 31 files mention `process`. The method path still answers
    // from `analyze_file` without parsing a single `ParsedDoc`, so the parse
    // count must not climb with the candidate count.
    let mut s = TestServer::with_fixture("references_stress").await;
    s.wait_for_index_ready().await;

    let text = target_text();
    s.open("src/Target.php", &text).await;
    let (line, col) = pos_of(&text, "process");

    let before = s.debug_stats_parses().await;
    let resp = s.references("src/Target.php", line, col, true).await;
    let after = s.debug_stats_parses().await;

    expect!["src/Target.php:11:20-11:27\nsrc/Target.php:13:41-13:48"]
        .assert_eq(&render_locations(&resp, &s.uri("")));
    assert!(
        after - before <= 2,
        "references parsed {} candidate docs; the method path must not parse \
         the text-matching workspace (30 noise files mention `process`)",
        after - before
    );
}

/// Slowness gap: public instance methods currently fall back to the full
/// workspace candidate set and rely on mir's internal text gate to reject files
/// that do not even spell the method token. That keeps results correct, but on
/// a cold large workspace it still means a broad mention-index pass before the
/// user sees final references. A host-side method-token candidate prefilter
/// should keep the first query proportional to files that can actually contain
/// the method reference.
#[tokio::test]
#[ignore = "known slowness gap: public instance-method references scan files that do not mention the method token"]
async fn references_public_instance_method_skips_files_without_member_token() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("Target.php"),
        "<?php\nclass Target {\n    public function pro$0cess(): void {}\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("Caller.php"),
        "<?php\n$t = new Target();\n$t->process();\n",
    )
    .unwrap();
    for i in 0..80 {
        std::fs::write(
            dir.path().join(format!("Noise{i}.php")),
            format!("<?php\nclass Noise{i} {{ public function idle(): void {{}} }}\n"),
        )
        .unwrap();
    }

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;
    server
        .open(
            "Target.php",
            "<?php\nclass Target {\n    public function process(): void {}\n}\n",
        )
        .await;

    let before = server.debug_stats_mir_mention_scans_recorded().await;
    let resp = server.references("Target.php", 2, 20, false).await;
    let after = server.debug_stats_mir_mention_scans_recorded().await;

    expect!["Caller.php:2:4-2:11"].assert_eq(&render_locations(&resp, &server.uri("")));
    let scans = after - before;
    assert!(
        scans <= 3,
        "public instance-method references scanned {scans} file texts; expected only \
         declaration/caller-sized work, not the 80 files that never mention `process`"
    );
}

#[tokio::test]
async fn references_protected_method_narrowed_to_hierarchy_stays_complete() {
    // Once the index is ready, `Base::boot` (protected) is narrowed to the
    // declaring file + its transitive subtype files. The narrowed search must
    // still find the in-class call and the subclass call, and must never reach
    // `Stranger::boot` (a same-named protected method on an unrelated class).
    let mut s = TestServer::with_fixture("references_protected").await;
    s.wait_for_index_ready().await;

    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/references_protected/src/Base.php");
    let text = std::fs::read_to_string(path).expect("read Base.php fixture");
    s.open("src/Base.php", &text).await;
    let (line, col) = pos_of(&text, "boot");

    let resp = s.references("src/Base.php", line, col, true).await;

    // Exact 5-location set: declaring file (decl + in-class call) plus each
    // subtype file's call — via plain extends, FQN extends, and an aliased
    // extends. A stray `Stranger.php` entry (unrelated same-named method) or
    // a dropped/duplicated subtype hit would show up as a wrong line here.
    expect![[r#"
        src/Aliased.php:10:15-10:19
        src/Base.php:12:15-12:19
        src/Base.php:6:23-6:27
        src/Child.php:8:15-8:19
        src/Grandchild.php:8:15-8:19"#]]
    .assert_eq(&render_locations(&resp, &s.uri("")));
}

#[tokio::test]
async fn edits_and_reads_take_bounded_reference_index_locks() {
    // References answer from mir's delta-maintained posting lists: an edit
    // commits the changed file's postings (a handful of locks), and a read is
    // a bounded per-key lookup. The counter must scale with the edit/read —
    // never with candidate-file count (which would mean per-request index
    // rebuilds crept back in).
    let mut s = TestServer::with_fixture("references_stress").await;
    s.wait_for_index_ready().await;

    let text = target_text();
    s.open("src/Target.php", &text).await;
    let (line, col) = pos_of(&text, "process");

    // Warm query so the candidate set is committed before measuring.
    let _ = s.references("src/Target.php", line, col, true).await;
    let before = s.debug_stats_ref_index_locks().await;

    // Edit path: a change triggers analysis + the dependent republish sweep.
    s.change("src/Target.php", 2, &format!("{text}\n// edited\n"))
        .await;
    let _ = s
        .client()
        .drain_publish_diagnostics_uris(tokio::time::Duration::from_millis(300))
        .await;
    // Read path: references over the full candidate set.
    let resp = s.references("src/Target.php", line, col, true).await;
    expect!["src/Target.php:11:20-11:27\nsrc/Target.php:13:41-13:48"]
        .assert_eq(&render_locations(&resp, &s.uri("")));

    let after = s.debug_stats_ref_index_locks().await;
    let taken = after - before;
    assert!(
        taken <= 64,
        "RefIndex was locked {taken} time(s) on one edit/read cycle; \
         expected a small bounded count, not per-candidate work"
    );
}

#[tokio::test]
async fn references_private_method_does_not_parse_candidate_files() {
    // `Target::compute` is private — narrowed to its declaring file. The
    // narrowing happens on the URL list *before* any parse, so neither the 30
    // noise files nor the scope filtering trigger a `ParsedDoc` parse.
    let mut s = TestServer::with_fixture("references_stress").await;
    s.wait_for_index_ready().await;

    let text = target_text();
    s.open("src/Target.php", &text).await;
    let (line, col) = pos_of(&text, "compute");

    let before = s.debug_stats_parses().await;
    let resp = s.references("src/Target.php", line, col, true).await;
    let after = s.debug_stats_parses().await;

    expect!["src/Target.php:13:22-13:29\nsrc/Target.php:6:21-6:28"]
        .assert_eq(&render_locations(&resp, &s.uri("")));
    assert!(
        after - before <= 2,
        "private references parsed {} candidate docs; narrowing must precede \
         (and elide) parsing",
        after - before
    );
}
