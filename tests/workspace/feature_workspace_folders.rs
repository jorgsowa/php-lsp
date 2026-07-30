//! Workspace folders and watched-file notifications: add/remove folders,
//! did{Create,Delete,Rename}Files, and edge cases from workspace-scan path.

use super::*;

use expect_test::expect;
use std::time::Duration;
use tower_lsp::lsp_types::Url;

const CREATED: u32 = 1;
const CHANGED: u32 = 2;
const DELETED: u32 = 3;

async fn poll_until_symbol_uri_contains(
    server: &mut TestServer,
    query: &str,
    needle: &str,
    timeout: Duration,
) {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let found = server.workspace_symbols(query).await["result"]
            .as_array()
            .map(|a| {
                a.iter().any(|s| {
                    s["location"]["uri"]
                        .as_str()
                        .map(|u| u.contains(needle))
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false);
        if found {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out after {:?} waiting for '{}' with URI containing '{}'",
            timeout,
            query,
            needle
        );
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
}

// ── workspace/didChangeWorkspaceFolders ───────────────────────────────────────

#[tokio::test]
async fn add_workspace_folder_indexes_php_classes() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    let tmp = tempfile::tempdir().expect("create TempDir");
    std::fs::write(
        tmp.path().join("ExtraWidget.php"),
        "<?php\nclass ExtraWidget {}\n",
    )
    .expect("write ExtraWidget.php");

    let folder_uri = Url::from_file_path(tmp.path())
        .expect("valid file URI")
        .to_string();

    server.add_workspace_folder(&folder_uri).await;
    server
        .wait_until_symbol_present("ExtraWidget", Duration::from_secs(5))
        .await;

    let resp = server.workspace_symbols("ExtraWidget").await;
    let out = render_workspace_symbols(&resp, &folder_uri);
    expect![[r#"Class       ExtraWidget @ ExtraWidget.php:1"#]].assert_eq(&out);
}

/// A runtime-added folder must honor `indexVendor: false` the same way the
/// initial-roots scan does. Regression test: `did_change_workspace_folders`
/// used to build its exclude list from the raw config, missing the
/// `vendor/` push that `handle_initialized` applies, so a folder added after
/// startup scanned (and mirrored) its entire vendor tree.
#[tokio::test]
async fn add_workspace_folder_honors_index_vendor_false() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    let tmp = tempfile::tempdir().expect("create TempDir");
    std::fs::write(
        tmp.path().join("MarkerClass.php"),
        "<?php\nclass MarkerClass {}\n",
    )
    .expect("write MarkerClass.php");
    std::fs::create_dir_all(tmp.path().join("vendor/acme/lib")).expect("create vendor dir");
    std::fs::write(
        tmp.path().join("vendor/acme/lib/VendoredThing.php"),
        "<?php\nclass VendoredThing {}\n",
    )
    .expect("write vendored file");

    let folder_uri = Url::from_file_path(tmp.path())
        .expect("valid file URI")
        .to_string();

    server.add_workspace_folder(&folder_uri).await;
    server
        .wait_until_symbol_present("MarkerClass", Duration::from_secs(5))
        .await;

    // The scan has completed (MarkerClass is indexed); vendor/ must have
    // been excluded from it by default.
    let out = server.snapshot_workspace_symbols("VendoredThing").await;
    expect!["<no symbols>"].assert_eq(&out);
}

/// A folder added at runtime must get the same warm-start disk-cache replay
/// the initial roots get. Regression test: `did_change_workspace_folders`
/// used to call only `scan_workspace` for a newly-added folder, never
/// `warm_start_indexes` — so its first references query paid an on-demand
/// analyze-and-commit instead of answering from replayed postings.
///
/// Proof strategy: warm a shared cache dir for `added_root` in a first
/// server launch, then in a second launch (rooted elsewhere, background
/// warm sweep disabled) add `added_root` at runtime and confirm a
/// references query on it takes only 1 additional mir reference-index lock
/// (a pure posting read) rather than 2 (a read plus the on-demand
/// analyze-and-commit write) — measured directly against this same
/// scenario with the fix reverted, which takes 2.
#[tokio::test]
async fn add_workspace_folder_replays_warm_start_postings() {
    let widget = "<?php\nclass Widget {\n    public function spin(): void {}\n}\n";
    let caller = "<?php\n$w = new Widget();\n$w->spin();\n";

    let added_root = tempfile::tempdir().expect("added-root tempdir");
    std::fs::write(added_root.path().join("widget.php"), widget).expect("write widget.php");
    std::fs::write(added_root.path().join("caller.php"), caller).expect("write caller.php");
    let cache_dir = tempfile::tempdir().expect("cache tempdir");

    let opts = |warm_analysis: bool| {
        serde_json::json!({
            "cachePath": cache_dir.path().to_str().unwrap(),
            "diagnostics": {"enabled": false},
            "phpVersion": "8.3",
            "warmAnalysis": warm_analysis,
        })
    };

    // First launch: `added_root` is the initial (only) root, so its warm
    // sweep commits reference postings to the shared cache dir on disk.
    {
        let mut s = TestServer::with_root_and_options(added_root.path(), opts(true)).await;
        s.wait_for_index_ready().await;
        assert!(
            s.wait_for_warm_sweeps(1).await,
            "warm sweep did not complete"
        );
    }

    // Second launch: a *different*, initially-empty root, warm sweep
    // disabled — `added_root` is added at runtime below.
    let empty_root = tempfile::tempdir().expect("empty-root tempdir");
    let mut s = TestServer::with_root_and_options(empty_root.path(), opts(false)).await;
    s.wait_for_index_ready().await;

    let folder_uri = Url::from_file_path(added_root.path())
        .expect("valid file URI")
        .to_string();
    s.add_workspace_folder(&folder_uri).await;
    s.wait_until_symbol_present("Widget", Duration::from_secs(5))
        .await;
    // wait_until_symbol_present only proves file mirroring finished; the
    // warm-start replay step runs immediately after, in the same spawned
    // task, before send_refresh_requests — give it a moment to finish too.
    tokio::time::sleep(Duration::from_millis(300)).await;

    let widget_abs = added_root.path().join("widget.php");
    let widget_path = widget_abs.to_str().expect("valid utf8 path");
    s.open(widget_path, widget).await;

    let before_locks = s.debug_stats_ref_index_locks().await;
    // Cursor on `spin` in its declaration (line 2, col 20).
    let resp = s.references(widget_path, 2, 20, false).await;
    let after_locks = s.debug_stats_ref_index_locks().await;

    let out = render_locations(&resp, &folder_uri);
    expect!["caller.php:2:4-2:8"].assert_eq(&out);
    assert_eq!(
        after_locks - before_locks,
        1,
        "references on a runtime-added folder must answer from a replayed \
         posting read (1 lock) — 2 means it fell back to an on-demand \
         analyze-and-commit, i.e. warm-start replay didn't happen"
    );
}

#[tokio::test]
async fn add_empty_workspace_folder_does_not_crash() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    let tmp = tempfile::tempdir().expect("create TempDir");
    let folder_uri = Url::from_file_path(tmp.path())
        .expect("valid file URI")
        .to_string();

    server.add_workspace_folder(&folder_uri).await;
    server
        .wait_until_symbol_present("User", Duration::from_secs(3))
        .await;

    let out = server.snapshot_workspace_symbols("User").await;
    expect![[r#"
        Class       User @ src/Model/User.php:4
        Property    $users @ src/Service/Registry.php:9"#]]
    .assert_eq(&out);

    let out = server.snapshot_workspace_symbols("NonExistent").await;
    expect![[r#"<no symbols>"#]].assert_eq(&out);
}

#[tokio::test]
async fn add_workspace_folder_idempotent_on_duplicate() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    let tmp = tempfile::tempdir().expect("create TempDir");
    std::fs::write(
        tmp.path().join("UniqueGadget.php"),
        "<?php\nclass UniqueGadget {}\n",
    )
    .expect("write UniqueGadget.php");

    let folder_uri = Url::from_file_path(tmp.path())
        .expect("valid file URI")
        .to_string();

    server.add_workspace_folder(&folder_uri).await;
    server.add_workspace_folder(&folder_uri).await;
    server
        .wait_until_symbol_present("UniqueGadget", Duration::from_secs(5))
        .await;

    let resp = server.workspace_symbols("UniqueGadget").await;
    let out = render_workspace_symbols(&resp, &folder_uri);
    expect![[r#"Class       UniqueGadget @ UniqueGadget.php:1"#]].assert_eq(&out);
}

/// `didChangeWorkspaceFolders` removal only drops the folder from the
/// tracked-roots list (`root_paths`) — it does not evict already-indexed
/// docs, so a removed folder's symbols stay queryable by design. This pins
/// that documented behavior; it is not evidence the removal did anything,
/// since a no-op handler would produce an identical result.
#[tokio::test]
async fn remove_workspace_folder_keeps_already_indexed_docs_queryable() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    let root_uri = server.uri("").trim_end_matches('/').to_string();
    server.remove_workspace_folder(&root_uri).await;

    let out = server.snapshot_workspace_symbols("User").await;
    expect![[r#"
        Class       User @ src/Model/User.php:4
        Property    $users @ src/Service/Registry.php:9"#]]
    .assert_eq(&out);
}

// ── workspace-scan edge cases ─────────────────────────────────────────────────

#[tokio::test]
async fn workspace_without_composer_json_still_works() {
    let mut server = TestServer::with_fixture("no-composer").await;
    server.wait_for_index_ready().await;

    let (text, line, ch) = server.locate("src/standalone.php", "standalone", 0);
    server.open("src/standalone.php", &text).await;
    let resp = server.hover("src/standalone.php", line, ch).await;
    let out = render_hover(&resp);
    expect![[r#"
        ```php
        function standalone(int $n): int
        ```"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn nonexistent_psr4_dir_does_not_crash_server() {
    let mut server = TestServer::with_fixture("missing-psr4-dir").await;
    server.wait_for_index_ready().await;

    let out = server.snapshot_workspace_symbols("Alive").await;
    expect!["Class       Alive @ src/Present/Alive.php:4"].assert_eq(&out);

    let (text, _, _) = server.locate("src/Present/Alive.php", "<?php", 0);
    server.open("src/Present/Alive.php", &text).await;
    let resp = server.document_symbols("src/Present/Alive.php").await;
    let out = render_document_symbols(&resp);
    expect![[r#"
        Class Alive @L4
          Method hello @L6"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn malformed_composer_json_does_not_crash_server() {
    let mut server = TestServer::with_fixture("broken-composer").await;
    server.wait_for_index_ready().await;

    let (text, _, _) = server.locate("src/Thing.php", "<?php", 0);
    server.open("src/Thing.php", &text).await;

    let resp = server.document_symbols("src/Thing.php").await;
    let out = render_document_symbols(&resp);
    expect![[r#"
        Class Thing @L4
          Method go @L6"#]]
    .assert_eq(&out);
}

/// A malformed `composer.json` must degrade to "no PSR-4 map" rather than
/// silently disabling undefined-class detection outright — a genuinely
/// missing class in the same workspace must still be flagged. Without this,
/// `malformed_composer_json_does_not_crash_server` above only proves the
/// server survives, not that diagnostics still function.
#[tokio::test]
async fn malformed_composer_json_still_flags_undefined_class() {
    let mut server = TestServer::with_fixture("broken-composer").await;
    server.wait_for_index_ready().await;

    server
        .check_diagnostics(
            r#"<?php
namespace App;
function _wrap(): void {
    $x = new TrulyNonExistentClass9z();
//           ^^^^^^^^^^^^^^^^^^^^^^^ error: TrulyNonExistentClass9z
}
"#,
        )
        .await;
}

/// A PSR-4 prefix whose base directory doesn't exist on disk
/// (`missing-psr4-dir`'s `Ghost\` -> `src/Ghost/`) must not suppress
/// undefined-class detection for that namespace — a reference to a
/// nonexistent `Ghost\` class must still be flagged, while the sibling
/// `Present\` mapping (whose directory does exist) keeps resolving normally.
/// `nonexistent_psr4_dir_does_not_crash_server` above only proves the server
/// survives; this proves the missing directory doesn't fail open.
#[tokio::test]
async fn nonexistent_psr4_dir_still_flags_undefined_class_in_that_namespace() {
    let mut server = TestServer::with_fixture("missing-psr4-dir").await;
    server.wait_for_index_ready().await;

    server
        .check_diagnostics(
            r#"<?php
namespace Consumer;

use Ghost\SomeClass;
use Present\Alive;

function make(): void {
    $a = new Alive();
    $g = new SomeClass();
//           ^^^^^^^^^ error: Ghost\SomeClass
}
"#,
        )
        .await;
}

// ── workspace/didCreateFiles / didDeleteFiles / didRenameFiles ────────────────

#[tokio::test]
async fn did_rename_files_updates_index_to_new_path() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    let old_uri = server.uri("src/Model/User.php");
    let new_uri = server.uri("src/Entity/User.php");

    let (content, _, _) = server.locate("src/Model/User.php", "<?php", 0);
    server.write_file("src/Entity/User.php", &content);
    server.remove_file("src/Model/User.php");

    server
        .did_rename_files(vec![(old_uri.clone(), new_uri.clone())])
        .await;

    poll_until_symbol_uri_contains(
        &mut server,
        "User",
        "Entity/User.php",
        Duration::from_secs(3),
    )
    .await;

    let out = server.snapshot_workspace_symbols("User").await;
    expect![[r#"
        Class       User @ src/Entity/User.php:4
        Property    $users @ src/Service/Registry.php:9"#]]
    .assert_eq(&out);
}

/// A renamed file's diagnostics under its old URI must be cleared, same as
/// did_delete_files — otherwise a client keeps showing stale diagnostics for
/// a path that no longer exists.
#[tokio::test]
async fn did_rename_files_clears_diagnostics_under_old_uri() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    let (content, _, _) = server.locate("src/Model/User.php", "<?php", 0);
    server.open("src/Model/User.php", &content).await;

    let old_uri = server.uri("src/Model/User.php");
    let new_uri = server.uri("src/Entity/User.php");

    server.write_file("src/Entity/User.php", &content);
    server.remove_file("src/Model/User.php");

    let results = server
        .did_rename_files(vec![(old_uri.clone(), new_uri.clone())])
        .await;

    let diag_notif = &results[0];
    let notif_uri = diag_notif["params"]["uri"].as_str().unwrap_or("");
    assert!(
        notif_uri.ends_with("Model/User.php"),
        "publishDiagnostics must be for the old URI, got: {notif_uri}"
    );
    expect!["<empty>"].assert_eq(&render_diagnostics_notification(diag_notif));
}

#[tokio::test]
async fn did_create_files_adds_new_class_to_index() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    expect!["<no symbols>"].assert_eq(&server.snapshot_workspace_symbols("OrderRepo").await);

    server.write_file(
        "src/Repository/OrderRepo.php",
        "<?php\nnamespace App\\Repository;\nclass OrderRepo {}\n",
    );
    let new_uri = server.uri("src/Repository/OrderRepo.php");
    server.did_create_files(vec![new_uri]).await;

    server
        .wait_until_symbol_present("OrderRepo", Duration::from_secs(3))
        .await;

    let out = server.snapshot_workspace_symbols("OrderRepo").await;
    expect!["Class       OrderRepo @ src/Repository/OrderRepo.php:2"].assert_eq(&out);
}

#[tokio::test]
async fn did_delete_files_removes_class_and_clears_diagnostics() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    let (content, _, _) = server.locate("src/Model/User.php", "<?php", 0);
    server.open("src/Model/User.php", &content).await;

    let uri = server.uri("src/Model/User.php");
    server.remove_file("src/Model/User.php");

    let results = server.did_delete_files(vec![uri]).await;

    let diag_notif = &results[0];
    let notif_uri = diag_notif["params"]["uri"].as_str().unwrap_or("");
    assert!(
        notif_uri.ends_with("Model/User.php"),
        "publishDiagnostics must be for User.php, got URI: {notif_uri}"
    );
    expect!["<empty>"].assert_eq(&render_diagnostics_notification(diag_notif));

    // `#class:` scopes the query to the Class kind — plain "User" would also
    // match Registry.php's `$users` property (a real, correct match since the
    // properties/constants workspace/symbol fix), which never goes away here.
    server
        .wait_until_symbol_absent("#class:User", Duration::from_secs(3))
        .await;

    expect!["<no symbols>"].assert_eq(&server.snapshot_workspace_symbols("#class:User").await);
}

// ── didChangeWatchedFiles edge cases ──────────────────────────────────────────

#[tokio::test]
async fn changed_event_does_not_overwrite_open_editor_file() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("editor.php"),
        "<?php\nfunction diskVersion(): void {}\n",
    )
    .unwrap();

    let mut server = TestServer::with_root(tmp.path()).await;
    server
        .open("editor.php", "<?php\nfunction editorVersion(): void {}\n")
        .await;

    let uri = server.uri("editor.php");
    server.did_change_watched_files(vec![(uri, CHANGED)]).await;

    let resp = server.hover("editor.php", 1, 10).await;
    let out = render_hover(&resp);
    expect![[r#"
        ```php
        function editorVersion(): void
        ```"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn batch_changes_all_applied() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    server.write_file(
        "src/Service/Alpha.php",
        "<?php\nnamespace App\\Service;\n\nclass Alpha {}\n",
    );
    server.write_file(
        "src/Service/Beta.php",
        "<?php\nnamespace App\\Service;\n\nclass Beta {}\n",
    );
    server.remove_file("src/Service/Registry.php");

    let alpha_uri = server.uri("src/Service/Alpha.php");
    let beta_uri = server.uri("src/Service/Beta.php");
    let registry_uri = server.uri("src/Service/Registry.php");

    server
        .did_change_watched_files(vec![
            (alpha_uri, CREATED),
            (beta_uri, CREATED),
            (registry_uri, DELETED),
        ])
        .await;

    server
        .wait_until_symbol_present("Alpha", Duration::from_secs(3))
        .await;
    server
        .wait_until_symbol_present("Beta", Duration::from_secs(3))
        .await;
    server
        .wait_until_symbol_absent("Registry", Duration::from_secs(3))
        .await;

    let alpha_out = server.snapshot_workspace_symbols("Alpha").await;
    expect![[r#"Class       Alpha @ src/Service/Alpha.php:3"#]].assert_eq(&alpha_out);

    let beta_out = server.snapshot_workspace_symbols("Beta").await;
    expect![[r#"Class       Beta @ src/Service/Beta.php:3"#]].assert_eq(&beta_out);

    let registry_out = server.snapshot_workspace_symbols("Registry").await;
    expect![[r#"<no symbols>"#]].assert_eq(&registry_out);
}
