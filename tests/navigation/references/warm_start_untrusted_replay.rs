//! Regression pin for ROADMAP item 0c / plan step 1 (`~/.claude/plans/crispy-noodling-key.md`).
//!
//! mir 0.67.0's `warm_start_files` returns the subset of replayed files whose
//! reference postings it can't fully trust — a disk-cache replay of a commit
//! whose issue set has an unresolved name is never immune to workspace-growth
//! invalidation, so the first live query to touch such a file pays a full
//! synchronous `analyze_file` (mir's changelog measured ~1.3-1.5s on a real
//! 15K-file workspace). Mir's own recommendation: hand that list to a
//! background `reanalyze_files_cancellable` call so the cost lands during
//! idle time after boot, not on the user's first request.
//!
//! `warm_start_indexes` (`src/document/document_store.rs`) currently
//! discards the returned list entirely. This test proves the gap via an
//! *absence* of background activity: with the ambient warm sweep disabled
//! (`warmAnalysis: false`), nothing else could reanalyze the untrusted file,
//! so `warm_start_untrusted_reanalyzed` must stay at 0 forever — until step 1
//! wires the list through, at which point it should tick up shortly after
//! `indexReady`, with no query ever issued.

use super::*;

use serde_json::json;

/// A class reference that can never resolve — the file's mir commit is
/// stamped `resolved: false` and stays that way across any number of
/// generation bumps (there's no `UndefinedThing` to ever load), which is
/// exactly the shape `warm_start_files` flags as untrusted on replay.
const CALLER: &str = "<?php\nclass Caller {\n    public function make(): UndefinedThing {\n        return new UndefinedThing();\n    }\n}\n";

#[tokio::test]
#[ignore = "warm_start_indexes doesn't wire up mir's untrusted-file list yet (ROADMAP 0c step 1)"]
async fn warm_start_reanalyzes_untrusted_file_in_background_without_a_query() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    let cache_dir = tempfile::tempdir().expect("cache tempdir");
    std::fs::write(workspace.path().join("caller.php"), CALLER).expect("write caller.php");

    let opts = |warm_analysis: bool| {
        json!({
            "cachePath": cache_dir.path().to_str().unwrap(),
            "diagnostics": {"enabled": false},
            "warmAnalysis": warm_analysis,
        })
    };

    // ── First launch: warm sweep commits caller.php with resolved: false ────
    {
        let mut s = TestServer::with_root_and_options(workspace.path(), opts(true)).await;
        s.wait_for_index_ready().await;
        assert!(
            s.wait_for_warm_sweeps(1).await,
            "warm sweep did not complete"
        );
        // Sweep flushes on completion — caller.php's untrusted commit is on disk.
    }

    // ── Second launch: ambient sweep OFF, so only warm_start's own wiring
    //    could touch caller.php. `indexReady` alone must trigger the
    //    background reanalysis — no `references()` call anywhere in this test.
    {
        let mut s = TestServer::with_root_and_options(workspace.path(), opts(false)).await;
        s.wait_for_index_ready().await;
        assert!(
            s.wait_for_warm_start_untrusted_reanalysis(1).await,
            "warm_start_indexes must hand mir's untrusted-file list to a \
             background reanalysis instead of discarding it"
        );
    }
}
