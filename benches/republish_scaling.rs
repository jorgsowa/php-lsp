//! Per-edit diagnostics-republish scaling guard (WS3 acceptance).
//!
//! Simulates the LSP edit hot path at growing workspace sizes and pins the
//! WS3 property: per-edit republish cost is O(open files) — flat as the
//! ingested-file count grows — because `reanalyze_files_cancellable`
//! re-analyzes only the caller's open set and salsa memoization absorbs the
//! rest. The superseded dependent-sweep path (`reanalyze_dependents`) is
//! measured alongside as the degradation reference: it rebuilds
//! `dependency_graph()` per edit, an O(all-ingested-files) walk.
//!
//! Synthetic workspace: `base.php` defines `Base`; every file extends it, so
//! the whole workspace is a dependent of every base edit — the worst case for
//! the old path and exactly the shape (edit a base class under many
//! dependents) users report as "the server slows down".
//!
//! Run with `cargo bench --bench republish_scaling`. Release mode matters.

use std::sync::Arc;
use std::time::{Duration, Instant};

use mir_analyzer::{AnalysisSession, IndexCancel, PhpVersion};

const SIZES: &[usize] = &[100, 1000, 5000];
const OPEN_FILES: usize = 4;
const EDITS: usize = 12;
const WARMUP_EDITS: usize = 4;
/// Memo-hit sweep ceiling: the no-edit sweep re-validates nothing, so it
/// must stay in microseconds at every size. Sub-0.1ms means make ratio
/// gates noise-dominated; an O(N) regression lands in milliseconds, so an
/// absolute ceiling is the robust guard.
const HIT_CEILING_MS: f64 = 1.0;
/// Post-edit sweep ceiling at any size. After a write, salsa re-validates
/// each open file's memo tree; measured at ~50µs per open file at 5000
/// ingested files (cache effects, not an O(N) walk — the memo-hit control
/// stays flat). Keep the ceiling tight enough that an O(N) regression
/// (the old dependent sweep was ~70ms here) trips it immediately.
const SWEEP_CEILING_MS: f64 = 2.0;

fn base_text(edit: usize) -> Arc<str> {
    Arc::from(format!(
        "<?php\nclass Base {{\n    public function ping(): int {{\n        $v = {edit};\n        return $v + 1;\n    }}\n}}\n"
    ))
}

fn dependent_text(i: usize) -> Arc<str> {
    Arc::from(format!(
        "<?php\nclass Dep{i} extends Base {{\n    public function go(): int {{\n        return $this->ping() + {i};\n    }}\n}}\n"
    ))
}

struct Workspace {
    session: AnalysisSession,
    base: Arc<str>,
    open_set: Vec<Arc<str>>,
}

/// Build a session with `size` ingested dependents, `OPEN_FILES` of which are
/// treated as the editor's open files.
fn build(size: usize) -> Workspace {
    let session = AnalysisSession::new(PhpVersion::LATEST);
    session.ensure_all_stubs();

    let base: Arc<str> = Arc::from("bench://base.php");
    session.ingest_file(base.clone(), base_text(0));

    let mut open_set: Vec<Arc<str>> = Vec::with_capacity(OPEN_FILES);
    for i in 0..size {
        let file: Arc<str> = Arc::from(format!("bench://dep{i}.php"));
        session.ingest_file(file.clone(), dependent_text(i));
        if i < OPEN_FILES {
            open_set.push(file);
        }
    }
    Workspace {
        session,
        base,
        open_set,
    }
}

fn mean_ms(samples: &[Duration]) -> f64 {
    samples.iter().sum::<Duration>().as_secs_f64() * 1000.0 / samples.len() as f64
}

/// Simulated keystrokes on the base file, each followed by the republish
/// sweep. Returns (ingest ms, sweep ms) means so a slope in either phase is
/// attributable.
fn measure_edits(ws: &Workspace, sweep: impl Fn()) -> (f64, f64) {
    let mut ingest = Vec::with_capacity(EDITS - WARMUP_EDITS);
    let mut sweeps = Vec::with_capacity(EDITS - WARMUP_EDITS);
    for edit in 0..EDITS {
        let t0 = Instant::now();
        ws.session.ingest_file(ws.base.clone(), base_text(edit + 1));
        let t1 = Instant::now();
        sweep();
        let t2 = Instant::now();
        if edit >= WARMUP_EDITS {
            ingest.push(t1 - t0);
            sweeps.push(t2 - t1);
        }
    }
    (mean_ms(&ingest), mean_ms(&sweeps))
}

fn main() {
    // Sessions here run mir's parallel analysis before any DocumentStore
    // exists — size the rayon stacks like production first.
    php_lsp::document_store::ensure_rayon_worker_stacks();
    println!(
        "republish scaling — per-edit phase means over {} edits",
        EDITS - WARMUP_EDITS
    );
    println!(
        "{:>8}  {:>11}  {:>16}  {:>11}  {:>11}  {:>17}",
        "files", "ingest ms", "open-sweep ms", "hit ms", "ingest ms", "dependent-sweep ms"
    );

    let mut hit_max = 0f64;
    let mut sweep_max = 0f64;
    for &size in SIZES {
        // New path: re-analyze exactly the open files; no dependency graph.
        let ws = build(size);
        let (open_ingest, open_sweep) = measure_edits(&ws, || {
            let analyses = ws
                .session
                .reanalyze_files_cancellable(&ws.open_set, &IndexCancel::new());
            std::hint::black_box(analyses);
        });
        // Control: the same sweep with no edit in between — a pure memo hit.
        // Separates salsa's post-edit re-validation cost from anything that
        // scales with workspace size on the read itself.
        let hit_sweep = {
            let mut samples = Vec::new();
            for i in 0..EDITS {
                let t0 = Instant::now();
                let analyses = ws
                    .session
                    .reanalyze_files_cancellable(&ws.open_set, &IndexCancel::new());
                std::hint::black_box(analyses);
                if i >= WARMUP_EDITS {
                    samples.push(t0.elapsed());
                }
            }
            mean_ms(&samples)
        };

        // Old path: compute + re-analyze the transitive dependents of base.
        let ws_old = build(size);
        let (dep_ingest, dep_sweep) = measure_edits(&ws_old, || {
            let analyses = ws_old.session.reanalyze_dependents(ws_old.base.as_ref());
            std::hint::black_box(analyses);
        });

        hit_max = hit_max.max(hit_sweep);
        sweep_max = sweep_max.max(open_sweep);
        println!(
            "{size:>8}  {open_ingest:>11.3}  {open_sweep:>16.3}  {hit_sweep:>11.3}  {dep_ingest:>11.3}  {dep_sweep:>17.3}"
        );
    }

    let hit_ok = hit_max <= HIT_CEILING_MS;
    let sweep_ok = sweep_max <= SWEEP_CEILING_MS;
    println!(
        "\nmemo-hit sweep max {hit_max:.3} ms (ceiling {HIT_CEILING_MS}): {}; post-edit sweep max {sweep_max:.3} ms (ceiling {SWEEP_CEILING_MS}): {}",
        if hit_ok {
            "OK"
        } else {
            "OVER — read path re-coupled to workspace size!"
        },
        if sweep_ok {
            "OK"
        } else {
            "OVER — O(N) work is back on the edit path!"
        },
    );
    if !hit_ok || !sweep_ok {
        std::process::exit(1);
    }
}
