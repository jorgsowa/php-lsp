//! End-to-end liveness of the edit → sweep → open pipeline.
//!
//! A `didChange` publishes the edited buffer and then sweeps every other open
//! buffer against a salsa snapshot; a `didOpen` arriving meanwhile mirrors its
//! text into the same db, and that write waits for outstanding snapshots.
//! Both sides must keep making progress under that overlap — a stall in
//! either one stops diagnostics for the whole session.
//!
//! This runs the editor's own shape (type in one file, open another the
//! moment its diagnostics land) and fails on the publish timeout instead of
//! hanging. The precise snapshot/lock interleaving that once deadlocked here
//! lives in the analyzer, and is pinned deterministically by mir's
//! `sweep_commit_does_not_deadlock_a_concurrent_file_write`.

use super::*;
use serde_json::json;

/// Edit/open pairs. Enough to cover many sweeps, few enough that the test
/// stays a couple of seconds on a loaded runner.
const ROUNDS: i32 = 16;
/// Buffers open before the burst, so every sweep re-analyses a real set.
const PRE_OPENED: usize = 8;

fn fixture_text(name: &str) -> String {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/references_stress/src")
        .join(name);
    std::fs::read_to_string(path).expect("read references_stress fixture")
}

/// A class body wide enough that re-analysing the open set takes real time,
/// so a `didOpen` sent right after an edit publishes lands inside that
/// edit's sweep rather than between two sweeps.
fn wide_body(class: &str, marker: i32) -> String {
    let mut src = format!("<?php\n\nnamespace App;\n\nclass {class}\n{{\n");
    src.push_str(&format!(
        "    private function compute(): int {{ return {marker}; }}\n"
    ));
    for m in 0..120 {
        src.push_str(&format!(
            "    public function process{m}(Target $t): int {{ return $t->process() + $this->compute() + {m}; }}\n"
        ));
    }
    src.push_str("}\n");
    src
}

#[tokio::test]
async fn interleaved_edits_and_opens_keep_publishing() {
    let mut s = TestServer::with_fixture("references_stress").await;
    s.wait_for_index_ready().await;

    let target = fixture_text("Target.php");
    s.open("src/Target.php", &target).await;
    for i in 0..PRE_OPENED {
        let name = format!("Noise{i}.php");
        s.open(&format!("src/{name}"), &wide_body(&format!("Noise{i}"), 0))
            .await;
    }

    // The edited file publishes before its dependent sweep starts, so an
    // open sent the moment those diagnostics land drops a salsa input write
    // into the middle of that sweep. The wide open set keeps each sweep long
    // enough for the open to arrive inside it.
    let target_uri = s.uri("src/Target.php");
    for round in 0..ROUNDS {
        let version = 2 + round;
        s.client()
            .notify(
                "textDocument/didChange",
                json!({
                    "textDocument": { "uri": target_uri, "version": version },
                    "contentChanges": [{ "text": format!("{target}\n// edit {round}\n") }],
                }),
            )
            .await;
        s.client()
            .wait_for_diagnostics_version(&target_uri, version)
            .await;

        // Opened against the in-flight sweep, and it must still publish.
        let name = format!("Noise{}.php", PRE_OPENED + round as usize);
        let uri = s.uri(&format!("src/{name}"));
        s.client()
            .notify(
                "textDocument/didOpen",
                json!({
                    "textDocument": {
                        "uri": uri,
                        "languageId": "php",
                        "version": 1,
                        "text": wide_body(&name.replace(".php", ""), round),
                    }
                }),
            )
            .await;
        s.client().wait_for_diagnostics_version(&uri, 1).await;
    }

    // And the server keeps serving db-backed requests afterwards.
    s.hover("src/Target.php", 4, 6).await;
}
