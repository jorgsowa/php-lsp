//! Protocol-wired regression pins for the cold (never-analyzed) candidate
//! path. Since mir 0.61 the host hands mir the whole workspace and mir gates
//! never-committed files on a whole-identifier, case-insensitive mention of
//! the symbol's name. Every test runs with `warmAnalysis: false` so unopened
//! files stay never-committed and the query genuinely exercises that gate —
//! these pin the bugs the old host-side text prefilter had:
//!
//! - case-sensitive scanning dropped `$s->PROCESS()` call sites entirely
//! - constructor references live at `new Cls(` sites that never spell
//!   `__construct`, so member-name-only gating would lose them
//! - a gated-out file must rejoin the candidate set after an edit

use super::*;

use expect_test::expect;

/// `warmAnalysis: false` server over `dir`: no background sweep ever commits
/// postings, so unopened files hit mir's cold-candidate gate on every query.
async fn cold_server(dir: &std::path::Path) -> TestServer {
    let mut server =
        TestServer::with_root_and_options(dir, serde_json::json!({ "warmAnalysis": false })).await;
    server.wait_for_index_ready().await;
    server
}

const SERVICE: &str = "<?php\nclass Service {\n    public function process(): void {}\n}\n";

/// A common method name across a workspace where most files only *textually*
/// near-miss it (substring identifiers, comments). The cold query must return
/// exactly the real call site — noise files can be analyzed or skipped, but
/// never produce phantom references, and the real one must not be lost.
#[tokio::test]
async fn cold_references_on_common_name_return_only_real_sites() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("svc.php"), SERVICE).unwrap();
    std::fs::write(
        dir.path().join("caller.php"),
        "<?php\nfunction run(Service $s): void {\n    $s->process();\n}\n",
    )
    .unwrap();
    // Substring near-misses: must not admit phantom refs.
    std::fs::write(
        dir.path().join("near_miss.php"),
        "<?php\nclass Batch {\n    public function processAll(): void {}\n}\n$unprocessed = 1;\n",
    )
    .unwrap();
    // Whole-word mention in a comment only: gate admits it (raw-text scan),
    // analysis then finds nothing — no phantom refs either.
    std::fs::write(
        dir.path().join("comment_only.php"),
        "<?php\n// process happens elsewhere\nclass Doc {}\n",
    )
    .unwrap();

    let mut server = cold_server(dir.path()).await;
    server.open("svc.php", SERVICE).await;

    let resp = server.references("svc.php", 2, 25, false).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");
    expect!["caller.php:2:8-2:15"].assert_eq(&render_locations(&resp, &server.uri("")));
}

/// PHP method dispatch is case-insensitive: `$s->PROCESS()` is a real call to
/// `Service::process`. The old host-side prefilter scanned case-sensitively,
/// so this file never entered the cold candidate set and the reference was
/// silently missing until a background sweep happened to commit the file.
#[tokio::test]
async fn cold_references_find_case_divergent_method_call() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("svc.php"), SERVICE).unwrap();
    std::fs::write(
        dir.path().join("shouty.php"),
        "<?php\nfunction go(Service $s): void {\n    $s->PROCESS();\n}\n",
    )
    .unwrap();

    let mut server = cold_server(dir.path()).await;
    server.open("svc.php", SERVICE).await;

    let resp = server.references("svc.php", 2, 25, false).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");
    expect!["shouty.php:2:8-2:15"].assert_eq(&render_locations(&resp, &server.uri("")));
}

const JOB: &str = "<?php\nclass Job {\n    public function __construct(private int $n) {}\n}\n";

/// Constructor references are recorded at `new Job(` sites, which never spell
/// `__construct` — the cold gate must admit files that only name the class,
/// or every instantiation site vanishes from a cold query.
#[tokio::test]
async fn cold_constructor_references_find_new_sites() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("job.php"), JOB).unwrap();
    std::fs::write(
        dir.path().join("spawn.php"),
        "<?php\nfunction spawn(): Job {\n    return new Job(1);\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("noise.php"),
        "<?php\nclass Unrelated {\n    public function work(): void {}\n}\n",
    )
    .unwrap();

    let mut server = cold_server(dir.path()).await;
    server.open("job.php", JOB).await;

    let resp = server.references("job.php", 2, 25, false).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");
    expect!["spawn.php:2:15-2:18"].assert_eq(&render_locations(&resp, &server.uri("")));
}

/// The gate skips analysis of a file that cannot name the symbol — it must
/// not freeze that file's state. An edit that introduces a call must be
/// reflected by the next query, with no warm sweep to paper over staleness.
#[tokio::test]
async fn cold_gated_file_joins_results_after_edit() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("svc.php"), SERVICE).unwrap();
    let idle_v1 = "<?php\nclass Idle {\n    public function tick(): void {}\n}\n";
    std::fs::write(dir.path().join("idle.php"), idle_v1).unwrap();

    let mut server = cold_server(dir.path()).await;
    server.open("svc.php", SERVICE).await;
    server.open("idle.php", idle_v1).await;

    let resp = server.references("svc.php", 2, 25, false).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");
    expect!["<none>"].assert_eq(&render_locations(&resp, &server.uri("")));

    server
        .change(
            "idle.php",
            2,
            "<?php\nclass Idle {\n    public function tick(Service $s): void {\n        $s->process();\n    }\n}\n",
        )
        .await;

    let resp = server.references("svc.php", 2, 25, false).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");
    expect!["idle.php:3:12-3:19"].assert_eq(&render_locations(&resp, &server.uri("")));
}

/// Rename shares the consolidated candidate path with references — a cold
/// rename must edit every case-exact call site across never-committed files.
///
/// Known-gap pin: the case-divergent `$s->PROCESS()` site in b.php is found
/// by find-references (PHP dispatch is case-insensitive) but deliberately
/// *not* edited — `narrow_range_to_word` documents that rename edits follow
/// the case-sensitive editor convention. If b.php ever appears in this
/// snapshot, that convention changed; update the docs alongside it.
#[tokio::test]
async fn cold_rename_edits_all_call_sites() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("svc.php"), SERVICE).unwrap();
    std::fs::write(
        dir.path().join("a.php"),
        "<?php\nfunction a(Service $s): void {\n    $s->process();\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("b.php"),
        "<?php\nfunction b(Service $s): void {\n    $s->PROCESS();\n}\n",
    )
    .unwrap();

    let mut server = cold_server(dir.path()).await;
    server.open("svc.php", SERVICE).await;

    let resp = server.rename("svc.php", 2, 25, "handle").await;
    assert!(resp["error"].is_null(), "rename error: {resp:?}");
    let snap = canonicalize_workspace_edit(&resp["result"], &server.uri(""));
    expect![[r#"
        // a.php
        2:8-2:15 → "handle"

        // svc.php
        2:20-2:27 → "handle""#]]
    .assert_eq(&snap);
}

/// Private methods keep the visibility fast path: the candidate scope is the
/// declaring file alone, so a cold query answers without touching the rest of
/// the workspace — and unrelated same-named calls stay excluded.
#[tokio::test]
async fn cold_private_method_references_stay_in_declaring_file() {
    let dir = tempfile::tempdir().unwrap();
    let vault = "<?php\nclass Vault {\n    private function open(): void {}\n    public function run(): void {\n        $this->open();\n    }\n}\n";
    std::fs::write(dir.path().join("vault.php"), vault).unwrap();
    std::fs::write(
        dir.path().join("other.php"),
        "<?php\nclass Door {\n    public function open(): void {}\n    public function slam(): void {\n        $this->open();\n    }\n}\n",
    )
    .unwrap();

    let mut server = cold_server(dir.path()).await;
    server.open("vault.php", vault).await;

    let resp = server.references("vault.php", 2, 26, false).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");
    expect!["<none>"].assert_eq(&render_locations(&resp, &server.uri("")));
}

/// Regression test for a false positive found 2026-07-21 verifying this bump
/// against a real Laravel-framework corpus: a `parent::method()` call inside
/// a class whose parent is *unresolved* (an external/vendor symbol php-lsp
/// never indexes — the common case for every real Composer project) was
/// wrongly treated as a candidate reference to ANY method sharing that bare
/// name, anywhere in the workspace. Reproduced live against
/// `benches/fixtures/laravel`: querying references on
/// `Illuminate\Database\Eloquent\Builder::__construct` returned `parent::
/// __construct()` call sites from completely unrelated classes (e.g.
/// `Illuminate\Console\Application`, which extends the unindexed
/// `Symfony\Component\Console\Application`) alongside the real results.
///
/// Root cause: mir's static-call analyzer recorded an unresolved receiver's
/// call under a bare, class-agnostic `methname:<name>` posting, and the
/// consolidated candidate path's empty-scoped-lookup fallback read that
/// bucket workspace-wide with no class affinity. Fixed in mir by scoping the
/// posting to the concrete (even if unresolved) receiver FQN instead — see
/// `static_call_on_unresolved_ancestor_does_not_collide_with_unrelated_class`
/// in mir's `crates/mir-analyzer/tests/indexed_queries.rs`.
#[tokio::test]
async fn cold_unresolved_parent_construct_is_not_a_false_positive_reference() {
    let dir = tempfile::tempdir().unwrap();
    let foo = "<?php\nclass Foo {\n    public function __construct() {\n    }\n}\n";
    std::fs::write(dir.path().join("foo.php"), foo).unwrap();
    std::fs::write(
        dir.path().join("child.php"),
        "<?php\nuse Some\\External\\Vendor\\BaseThing;\n\nclass Child extends BaseThing {\n    public function __construct() {\n        parent::__construct();\n    }\n}\n",
    )
    .unwrap();

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;
    server.open("foo.php", foo).await;

    let resp = server.references("foo.php", 2, 25, true).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");
    // `child.php`'s `parent::__construct()` must not appear — `Child` extends
    // the unrelated, unresolved `BaseThing`, not `Foo`.
    expect!["foo.php:2:20-2:31"].assert_eq(&render_locations(&resp, &server.uri("")));
}
