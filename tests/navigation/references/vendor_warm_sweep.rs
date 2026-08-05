//! Regression pin for ROADMAP item 0c / plan step 2 (`~/.claude/plans/crispy-noodling-key.md`).
//!
//! The ambient warm-analysis sweep skips `vendor/` unconditionally
//! (`DocumentStore::sweep_candidate_files`, `!is_vendor_uri(...)`) — a
//! deliberate default (see `LspConfig::index_vendor`'s docs) since sweeping
//! the whole vendor tree multiplies background CPU cost by vendor's file
//! count. But for a symbol whose candidate scope genuinely includes vendor
//! (a *vendor-defined* class/interface — builtins are handled separately by
//! plan step 0's scoping fix), never warming vendor means the first
//! reference query against it always pays a cold, synchronous, multi-second
//! analysis of every never-seen vendor file — this is the primary mechanism
//! behind the `Closure`/`ReflectionParameter` latency in the original bug
//! report.
//!
//! `warmVendorAnalysis: true` is meant to opt into a separate, throttled,
//! idle-priority sweep that ref-analyzes vendor files in the background.
//! Neither the config field's effect nor the sweep exist yet — this test
//! proves it via absence: with the flag on and a single vendor file present,
//! nothing ever increments `vendor_warm_sweeps_completed`, so waiting for it
//! always times out.

use super::*;

use serde_json::json;

const VENDOR_WIDGET: &str =
    "<?php\nnamespace Acme\\Lib;\n\nclass Widget {\n    public function spin(): void {}\n}\n";

#[tokio::test]
#[ignore = "warmVendorAnalysis sweep not implemented yet (ROADMAP 0c step 2)"]
async fn vendor_warm_sweep_completes_when_opted_in() {
    let dir = tempfile::tempdir().expect("workspace tempdir");
    std::fs::create_dir_all(dir.path().join("vendor/acme/lib/src")).expect("mkdir vendor");
    std::fs::write(
        dir.path().join("composer.json"),
        r#"{"autoload":{"psr-4":{"Acme\\Lib\\":"vendor/acme/lib/src/"}}}"#,
    )
    .expect("write composer.json");
    std::fs::write(
        dir.path().join("vendor/acme/lib/src/Widget.php"),
        VENDOR_WIDGET,
    )
    .expect("write Widget.php");

    let opts = json!({
        "warmVendorAnalysis": true,
        "diagnostics": {"enabled": false},
    });
    let mut server = TestServer::with_root_and_options(dir.path(), opts).await;
    server.wait_for_index_ready().await;

    // No `references()` call anywhere in this test — the sweep must run on
    // its own, the same way the main warm sweep does after `indexReady`.
    assert!(
        server.wait_for_vendor_warm_sweeps(1).await,
        "warmVendorAnalysis: true must eventually run a vendor warm-analysis \
         sweep to completion, even with no explicit request touching vendor"
    );
}
