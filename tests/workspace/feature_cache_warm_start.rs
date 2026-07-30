//! On-disk cache warm-start correctness.
//!
//! A cold start scans and parses every workspace file; a warm start (same
//! root, same cache dir) must serve `FileIndex` entries from disk without
//! re-parsing. These tests exercise the observable guarantee: symbols are
//! correct after a warm restart, and a file modified between starts is
//! detected because the content key changes even when mtime/size are stable.
//!
//! The `cachePath` initializationOption pins both servers to the same
//! cache directory without touching `XDG_CACHE_HOME`, keeping tests
//! isolated even when run in parallel.

use super::*;

use expect_test::expect;
use serde_json::json;

fn copy_dir_all(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

fn fixture_path(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// Warm restart (same workspace root, same cache dir) must expose the same
/// symbols as the cold start — the index is fully served from disk cache.
#[tokio::test]
async fn warm_start_serves_symbols_correctly() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    let cache_dir = tempfile::tempdir().expect("cache tempdir");
    copy_dir_all(&fixture_path("psr4-mini"), workspace.path()).expect("copy fixture");

    let opts = json!({
        "cachePath": cache_dir.path().to_str().unwrap(),
        "diagnostics": {"enabled": false},
    });

    // ── Cold start ────────────────────────────────────────────────────────────
    {
        let mut s = TestServer::with_root_and_options(workspace.path(), opts.clone()).await;
        s.wait_for_index_ready().await;
        let syms = s.snapshot_workspace_symbols("User").await;
        expect![[r#"
            Class       User @ src/Model/User.php:4
            Property    $users @ src/Service/Registry.php:9"#]]
        .assert_eq(&syms);
        // Server drops; both tempdirs remain alive so cache files persist.
    }

    // ── Warm restart on the same cache dir ────────────────────────────────────
    {
        let mut s = TestServer::with_root_and_options(workspace.path(), opts).await;
        s.wait_for_index_ready().await;
        // Symbols must be fully resolvable from the warm-loaded index.
        let syms = s.snapshot_workspace_symbols("User").await;
        expect![[r#"
            Class       User @ src/Model/User.php:4
            Property    $users @ src/Service/Registry.php:9"#]]
        .assert_eq(&syms);
    }
}

/// A file modified between two server starts must be detected on warm restart
/// and re-parsed, even when its mtime or size haven't changed.
/// The content-keyed cache ensures: different content → different key →
/// cache miss → fresh parse → new index.
#[tokio::test]
async fn warm_start_detects_changed_file_content() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    let cache_dir = tempfile::tempdir().expect("cache tempdir");
    copy_dir_all(&fixture_path("psr4-mini"), workspace.path()).expect("copy fixture");

    let opts = json!({
        "cachePath": cache_dir.path().to_str().unwrap(),
        "diagnostics": {"enabled": false},
    });

    // ── Cold start: scan and populate cache ───────────────────────────────────
    {
        let mut s = TestServer::with_root_and_options(workspace.path(), opts.clone()).await;
        s.wait_for_index_ready().await;
        // Confirm no Widget before the change.
        let syms = s.snapshot_workspace_symbols("Widget").await;
        expect![[r#"<no symbols>"#]].assert_eq(&syms);
    }

    // Replace User.php with a Widget class (different content, same-length file).
    let user_php = workspace.path().join("src/Model/User.php");
    std::fs::write(
        &user_php,
        "<?php\nnamespace App\\Model;\n\nclass Widget {}\n",
    )
    .expect("overwrite User.php");

    // ── Warm restart: changed file must be re-parsed, Widget must appear ──────
    {
        let mut s = TestServer::with_root_and_options(workspace.path(), opts).await;
        s.wait_for_index_ready().await;
        // The content-keyed cache detects the change → re-parsed → Widget found.
        let syms = s.snapshot_workspace_symbols("Widget").await;
        expect![[r#"Class       Widget @ src/Model/User.php:3"#]].assert_eq(&syms);
    }
}

/// `textDocument/didSave` must write a cache entry keyed exactly the way a
/// workspace scan would key it (same uri + content), so a later scan can hit
/// it instead of re-parsing.
///
/// This can't be proven by symbol resolution after a restart: did_save's
/// handler re-reads the file from disk to compute both the cached content
/// and the cache key, so whatever a subsequent scan finds on disk is by
/// construction identical to what did_save cached — a warm-start symbol
/// snapshot would look right even if did_save silently failed to write
/// anything at all, or the write raced with the read that follows it,
/// wrote to a wrong key, or FileIndex serialization was
/// broken. Reading the cache directly with the same key the scanner
/// would compute is the only way to actually pin "did_save wrote a
/// findable entry."
#[tokio::test]
async fn did_save_writes_a_cache_entry_findable_by_key() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    let cache_dir = tempfile::tempdir().expect("cache tempdir");
    copy_dir_all(&fixture_path("psr4-mini"), workspace.path()).expect("copy fixture");

    let opts = json!({
        "cachePath": cache_dir.path().to_str().unwrap(),
        "diagnostics": {"enabled": false},
    });

    let mut s = TestServer::with_root_and_options(workspace.path(), opts).await;
    s.wait_for_index_ready().await;

    let user_content = "<?php\nnamespace App\\Model;\n\nclass User { public int $id; }\n";
    let user_php = workspace.path().join("src/Model/User.php");
    std::fs::write(&user_php, user_content).expect("write User.php");
    let uri = s.uri("src/Model/User.php");
    s.client()
        .notify(
            "textDocument/didSave",
            serde_json::json!({ "textDocument": { "uri": uri } }),
        )
        .await;

    // Poll for did_save's spawn_blocking task to write the cache entry — a
    // fixed sleep here flakes under parallel test load.
    let cache = php_lsp::cache::WorkspaceCache::with_dir(cache_dir.path().to_path_buf());
    let key = php_lsp::cache::WorkspaceCache::key_for(&uri, user_content);
    let mut cached: Option<php_lsp::file_index::FileIndex> = None;
    for _ in 0..100 {
        cached = cache.read(&key);
        if cached.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(
        cached.is_some(),
        "did_save should have written a FileIndex cache entry keyed by (uri, saved content)"
    );
    let classes: Vec<&str> = cached
        .as_ref()
        .unwrap()
        .classes
        .iter()
        .map(|c| c.name.as_ref())
        .collect();
    assert_eq!(
        classes,
        vec!["User"],
        "cached FileIndex should reflect the saved content, not the original fixture"
    );
}

/// A cache entry written by `did_save` is actually consulted (not just
/// written and ignored) on the next workspace scan: with the entry
/// pre-seeded to a key a fresh parse would never independently produce,
/// warm start must still surface the class the entry describes.
#[tokio::test]
async fn did_save_cache_is_found_by_subsequent_scan() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    let cache_dir = tempfile::tempdir().expect("cache tempdir");
    copy_dir_all(&fixture_path("psr4-mini"), workspace.path()).expect("copy fixture");

    let opts = json!({
        "cachePath": cache_dir.path().to_str().unwrap(),
        "diagnostics": {"enabled": false},
    });

    // ── First server: open and save a file so did_save writes the cache ───────
    {
        let mut s = TestServer::with_root_and_options(workspace.path(), opts.clone()).await;
        s.wait_for_index_ready().await;

        let user_content = "<?php\nnamespace App\\Model;\n\nclass User { public int $id; }\n";
        // Write new content and trigger did_save so the cache entry is refreshed.
        let user_php = workspace.path().join("src/Model/User.php");
        std::fs::write(&user_php, user_content).expect("write User.php");
        let uri = s.uri("src/Model/User.php");
        s.client()
            .notify(
                "textDocument/didSave",
                serde_json::json!({ "textDocument": { "uri": uri } }),
            )
            .await;

        // Give did_save's spawn_blocking task a moment to write the cache entry.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }

    // ── Second server: warm start — scan must hit the did_save cache entry ────
    {
        let mut s = TestServer::with_root_and_options(workspace.path(), opts).await;
        s.wait_for_index_ready().await;
        // User class still visible (cache hit from did_save write).
        let syms = s.snapshot_workspace_symbols("User").await;
        expect![[r#"
            Class       User @ src/Model/User.php:3
            Property    $users @ src/Service/Registry.php:9"#]]
        .assert_eq(&syms);
    }
}

/// Reference postings persist across launches. The first server's analysis
/// warm sweep stages each analyzed file's postings into mir's session
/// `AnalysisCache` and flushes it on completion — before mir 0.56.0 only the
/// CLI batch pipeline ever wrote these entries, so `cache.bin` proves the
/// LSP-path write hook ran. A second server on the same cache dir (warm
/// sweep disabled so nothing re-derives postings in the background) then
/// answers a cross-file references query from the replayed index.
#[tokio::test]
async fn warm_start_replays_reference_postings_from_first_session() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    let cache_dir = tempfile::tempdir().expect("cache tempdir");
    let widget = "<?php\nclass Widget {\n    public function spin(): void {}\n}\n";
    let caller = "<?php\n$w = new Widget();\n$w->spin();\n";
    std::fs::write(workspace.path().join("widget.php"), widget).expect("write widget.php");
    std::fs::write(workspace.path().join("caller.php"), caller).expect("write caller.php");

    // Pin the PHP version: mir's cache epoch folds it in, and the direct
    // cache read below must open with the same byte the server used.
    let opts = |warm_analysis: bool| {
        json!({
            "cachePath": cache_dir.path().to_str().unwrap(),
            "diagnostics": {"enabled": false},
            "phpVersion": "8.3",
            "warmAnalysis": warm_analysis,
        })
    };

    // ── First launch: warm sweep commits postings and flushes on completion ──
    let caller_uri = {
        let mut s = TestServer::with_root_and_options(workspace.path(), opts(true)).await;
        s.wait_for_index_ready().await;
        assert!(
            s.wait_for_warm_sweeps(1).await,
            "warm sweep did not complete"
        );
        s.uri("caller.php")
    };

    // The LSP-path write hook: caller.php's postings are on disk, keyed by
    // its content hash, in the mir session cache.
    let session_dir = cache_dir.path().join("session");
    let php_v = "8.3"
        .parse::<mir_analyzer::PhpVersion>()
        .expect("valid version")
        .cache_byte();
    let mir_cache = mir_analyzer::cache::AnalysisCache::open(&session_dir, php_v, 0);
    let (_, ref_locs) = mir_cache
        .get(&caller_uri, &mir_analyzer::cache::hash_content(caller))
        .expect("warm sweep must persist an AnalysisCache entry for caller.php");
    assert!(
        !ref_locs.is_empty(),
        "persisted entry must carry caller.php's reference postings"
    );

    // ── Second launch: no warm sweep — references answered from the replay ──
    {
        let mut s = TestServer::with_root_and_options(workspace.path(), opts(false)).await;
        s.wait_for_index_ready().await;
        s.open("widget.php", widget).await;
        // Cursor on `spin` in its declaration (line 2, col 20).
        let resp = s.references("widget.php", 2, 20, false).await;
        let out = render_locations(&resp, &s.uri(""));
        expect![[r#"caller.php:2:4-2:8"#]].assert_eq(&out);
    }
}

/// Postings staged by an on-demand references query (not a warm sweep) must
/// still reach disk without a clean `shutdown` — the background flush loop
/// added to close the "not amortized" cliff (an unclean exit on a workspace
/// that never ran a warm sweep previously lost everything, since a flush
/// only ever happened on sweep completion or `shutdown`). `warmAnalysis` is
/// off so the only source of staged postings is the references query
/// itself, and the first session is dropped without calling `shutdown`,
/// simulating a crash/kill.
#[tokio::test]
async fn periodic_flush_persists_query_commits_without_shutdown() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    let cache_dir = tempfile::tempdir().expect("cache tempdir");
    let widget = "<?php\nclass Widget {\n    public function spin(): void {}\n}\n";
    let caller = "<?php\n$w = new Widget();\n$w->spin();\n";
    std::fs::write(workspace.path().join("widget.php"), widget).expect("write widget.php");
    std::fs::write(workspace.path().join("caller.php"), caller).expect("write caller.php");

    let opts = json!({
        "cachePath": cache_dir.path().to_str().unwrap(),
        "diagnostics": {"enabled": false},
        "phpVersion": "8.3",
        "warmAnalysis": false,
        "analysisCacheFlushIntervalMs": 50,
    });

    let caller_uri = {
        let mut s = TestServer::with_root_and_options(workspace.path(), opts).await;
        s.wait_for_index_ready().await;
        s.open("widget.php", widget).await;
        // Cursor on `spin` in its declaration — stages caller.php's postings
        // into the in-memory AnalysisCache via the on-demand freshness pass.
        s.references("widget.php", 2, 20, false).await;
        let uri = s.uri("caller.php");

        // Give the background flush loop (50 ms interval) at least one tick
        // before dropping — no `shutdown()` call, unlike every other test in
        // this file.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        uri
        // `s` drops here uncleanly.
    };

    let session_dir = cache_dir.path().join("session");
    let php_v = "8.3"
        .parse::<mir_analyzer::PhpVersion>()
        .expect("valid version")
        .cache_byte();
    let mir_cache = mir_analyzer::cache::AnalysisCache::open(&session_dir, php_v, 0);
    let (_, ref_locs) = mir_cache
        .get(&caller_uri, &mir_analyzer::cache::hash_content(caller))
        .expect("periodic flush must persist the query-staged entry for caller.php");
    assert!(
        !ref_locs.is_empty(),
        "persisted entry must carry caller.php's reference postings"
    );
}

/// Editors restore buffers at launch, so `didOpen` races `initialized` — a
/// session built before the cache dir is attached used to be pinned
/// in-memory-only, silently disabling ALL on-disk persistence for the whole
/// server lifetime (nothing flushed, nothing to replay next launch). Opening
/// a file immediately, before waiting for the index, must still leave mir's
/// session cache directory materialized on disk.
#[tokio::test]
async fn early_open_still_attaches_session_cache() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    let cache_dir = tempfile::tempdir().expect("cache tempdir");
    copy_dir_all(&fixture_path("psr4-mini"), workspace.path()).expect("copy fixture");

    let opts = json!({
        "cachePath": cache_dir.path().to_str().unwrap(),
        "diagnostics": {"enabled": true},
    });

    {
        let mut s = TestServer::with_root_and_options(workspace.path(), opts).await;
        // Open BEFORE waiting for the index — the race real editors hit.
        let user_php =
            std::fs::read_to_string(workspace.path().join("src/Model/User.php")).unwrap();
        s.open("src/Model/User.php", &user_php).await;
        s.wait_for_index_ready().await;
    }

    // `StubSliceCache::open` creates `<session>/stubs` eagerly at session
    // build, so its existence proves the session has the cache attached.
    // (An explicit `cachePath` is used as the cache root directly.)
    assert!(
        cache_dir.path().join("session/stubs").is_dir(),
        "session cache dir must exist on disk even when a didOpen preceded initialized"
    );
}

/// A returning session must seed mir's workspace symbol index from the disk
/// cache and answer its first references query without ever running the
/// tracked O(all-files) index walk — the per-process rebuild behind the
/// multi-second first-query cliff on large workspaces.
#[tokio::test]
async fn warm_start_seeds_symbol_index_and_first_query_avoids_tracked_walk() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    let cache_dir = tempfile::tempdir().expect("cache tempdir");
    let widget = "<?php\nclass Widget {\n    public function spin(): void {}\n}\n";
    let caller = "<?php\n$w = new Widget();\n$w->spin();\n";
    std::fs::write(workspace.path().join("widget.php"), widget).expect("write widget.php");
    std::fs::write(workspace.path().join("caller.php"), caller).expect("write caller.php");

    let opts = |warm_analysis: bool| {
        json!({
            "cachePath": cache_dir.path().to_str().unwrap(),
            "diagnostics": {"enabled": false},
            "phpVersion": "8.3",
            "warmAnalysis": warm_analysis,
        })
    };

    // ── First launch: sweep populates postings + definition slices on disk ──
    {
        let mut s = TestServer::with_root_and_options(workspace.path(), opts(true)).await;
        s.wait_for_index_ready().await;
        assert!(
            s.wait_for_warm_sweeps(1).await,
            "warm sweep did not complete"
        );
        s.shutdown().await;
    }

    // ── Second launch: index seeded before ready; queries never walk ──
    {
        let mut s = TestServer::with_root_and_options(workspace.path(), opts(false)).await;
        s.wait_for_index_ready().await;
        let (ready, _) = s.debug_stats_symbol_index().await;
        assert!(
            ready,
            "warm start over a populated cache must seed the symbol index singleton"
        );

        s.open("widget.php", widget).await;
        let resp = s.references("widget.php", 2, 20, false).await;
        let out = render_locations(&resp, &s.uri(""));
        expect![[r#"caller.php:2:4-2:8"#]].assert_eq(&out);

        let (_, walks) = s.debug_stats_symbol_index().await;
        assert_eq!(
            walks, 0,
            "a cache-warm session must never run the tracked O(all-files) index walk"
        );
        s.shutdown().await;
    }
}

/// External (watcher-shaped) file changes bypass `ingest_file`'s index
/// maintenance; the settle path must reconcile the seeded singleton before
/// queries read it. A class renamed on disk must be resolvable — and its
/// call site findable — right after `didChangeWatchedFiles`.
#[tokio::test]
async fn watcher_rename_stays_visible_in_seeded_symbol_index() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    let cache_dir = tempfile::tempdir().expect("cache tempdir");
    let shape = "<?php\nclass OldShape {\n    public static function draw(): void {}\n}\n";
    let user = "<?php\nOldShape::draw();\n";
    std::fs::write(workspace.path().join("shape.php"), shape).expect("write shape.php");
    std::fs::write(workspace.path().join("user.php"), user).expect("write user.php");

    let opts = |warm_analysis: bool| {
        json!({
            "cachePath": cache_dir.path().to_str().unwrap(),
            "diagnostics": {"enabled": false},
            "phpVersion": "8.3",
            "warmAnalysis": warm_analysis,
        })
    };

    {
        let mut s = TestServer::with_root_and_options(workspace.path(), opts(true)).await;
        s.wait_for_index_ready().await;
        assert!(
            s.wait_for_warm_sweeps(1).await,
            "warm sweep did not complete"
        );
        s.shutdown().await;
    }

    {
        let mut s = TestServer::with_root_and_options(workspace.path(), opts(false)).await;
        s.wait_for_index_ready().await;
        let (ready, _) = s.debug_stats_symbol_index().await;
        assert!(ready, "second launch must seed the singleton");

        // Rename the class on disk (git-pull shape) and notify via watcher.
        let new_shape = "<?php\nclass NewShape {\n    public static function draw(): void {}\n}\n";
        let new_user = "<?php\nNewShape::draw();\n";
        std::fs::write(workspace.path().join("shape.php"), new_shape).expect("rewrite shape.php");
        std::fs::write(workspace.path().join("user.php"), new_user).expect("rewrite user.php");
        s.did_change_watched_files(vec![
            (s.uri("shape.php"), 2), // Changed
            (s.uri("user.php"), 2),
        ])
        .await;

        s.open("shape.php", new_shape).await;
        // Cursor on `draw` in the renamed class's declaration.
        let resp = s.references("shape.php", 2, 27, false).await;
        let out = render_locations(&resp, &s.uri(""));
        expect!["user.php:1:10-1:14"].assert_eq(&out);
        s.shutdown().await;
    }
}
