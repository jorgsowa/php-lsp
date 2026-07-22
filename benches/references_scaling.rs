//! Fixture-free find-references improvement benchmark.
//!
//! Models the real cost driver behind the app-server verification's 5-41s
//! references cliff on generic short names (`Color`, `Node`, `Asset`, ...): a
//! *common* public method name (`process`) shared across unrelated classes,
//! where only a fraction of the text-matching files actually reference the
//! target `App\Service`.
//!
//! Neither the pre-0.61 host-side gate (`candidate_urls_for`, deleted in
//! `85880a6`) nor mir 0.61's own internal cold-candidate gate
//! (`indexed_references_to`'s `reference_gate_needles`) filter on more than
//! the bare symbol name — both admit every candidate that merely text-matches
//! `process`, including files that only define their *own*, unrelated
//! `process()` method. This bench does not compare two real code paths; it
//! measures the *unimplemented* upper bound:
//!
//!   BEFORE — hand mir the whole workspace; its gate admits every file that
//!     text-matches the method name (what ships today).
//!   AFTER  — additionally require the file to mention the owner class
//!     `Service` (a reachability gate nothing currently implements).
//!
//! Both produce the same references (a file that never names `Service` can't
//! resolve `Service::process`), so AFTER is pure, currently-unrealized
//! speedup. Reports the cold (first-query) latency the user feels, and the
//! SESSION axis for regression.
//!
//! Run: `cargo bench --bench references_scaling`

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mir_analyzer::Name;
use php_lsp::document_store::DocumentStore;
use tower_lsp::lsp_types::Url;

const HOT_METHOD: &str = "process";
const OWNER: &str = "Service";
/// 1 in N files actually references `App\Service`; the rest define/call their
/// own `process()` (text-match the method, never name the owner).
const REACH_EVERY: usize = 10;

fn service_file() -> (Url, String) {
    let url = Url::parse("file:///synth/Service.php").unwrap();
    let text = format!(
        "<?php\nnamespace App;\nclass {OWNER} {{\n    public function {HOT_METHOD}(): void {{}}\n}}\n"
    );
    (url, text)
}

/// Filler so each file is a representative size (~24 methods with real bodies
/// for `analyze_file` to infer), not a toy — otherwise a fixed setup cost swamps
/// the per-candidate analysis the pre-filter actually removes.
fn filler() -> String {
    let mut s = String::new();
    for j in 0..24 {
        s.push_str(&format!(
            "    public function helper{j}(int $x, string $s): int {{\n\
             \x20       $y = $x * {j} + strlen($s);\n\
             \x20       return $y > 0 ? $y : -$y;\n\
             \x20   }}\n"
        ));
    }
    s
}

/// References `App\Service::process` — names the owner (type hint) and calls it.
fn reachable_file(i: usize) -> (Url, String) {
    let url = Url::parse(&format!("file:///synth/R{i}.php")).unwrap();
    let text = format!(
        "<?php\nnamespace App;\n\
         class R{i} {{\n\
         \x20   private {OWNER} $svc;\n\
         \x20   public function run(int $a): int {{\n\
         \x20       $this->svc->{HOT_METHOD}();\n\
         \x20       return $a + {i};\n\
         \x20   }}\n{}}}\n",
        filler()
    );
    (url, text)
}

/// Noise: defines and calls its *own* `process()` — text-matches the method
/// name but never names `Service`, so it cannot resolve to `Service::process`.
fn noise_file(i: usize) -> (Url, String) {
    let url = Url::parse(&format!("file:///synth/N{i}.php")).unwrap();
    let text = format!(
        "<?php\nnamespace App;\n\
         class N{i} {{\n\
         \x20   public function {HOT_METHOD}(): void {{}}\n\
         \x20   public function run(int $a): int {{\n\
         \x20       $this->{HOT_METHOD}();\n\
         \x20       return $a + {i};\n\
         \x20   }}\n{}}}\n",
        filler()
    );
    (url, text)
}

/// Returns the store plus the set of URLs that name the owner (the reachable
/// subset the pre-filter keeps).
fn build(n: usize) -> (DocumentStore, HashSet<String>) {
    let store = DocumentStore::new();
    let mut reachable = HashSet::new();
    let (su, st) = service_file();
    reachable.insert(su.as_str().to_string());
    store.ingest(su, &st);
    for i in 0..n.saturating_sub(1) {
        let (u, t) = if i % REACH_EVERY == 0 {
            let f = reachable_file(i);
            reachable.insert(f.0.as_str().to_string());
            f
        } else {
            noise_file(i)
        };
        store.ingest(u, &t);
    }
    store.mark_index_ready();
    (store, reachable)
}

fn median_ms(mut s: Vec<Duration>) -> f64 {
    s.sort();
    s[s.len() / 2].as_secs_f64() * 1000.0
}

/// Median cold latency: a fresh store per rep so `analyze_file` is never a memo
/// hit. `select` picks the candidate subset from the freshly-built store.
fn cold_ms(
    n: usize,
    reps: usize,
    sym: &Name,
    select: impl Fn(&DocumentStore, &HashSet<String>) -> Vec<Arc<str>>,
) -> (usize, f64) {
    let mut samples = Vec::with_capacity(reps);
    let mut count = 0;
    for _ in 0..reps {
        let (store, reachable) = build(n);
        let files = select(&store, &reachable);
        count = files.len();
        let t = Instant::now();
        std::hint::black_box(store.indexed_references(sym, &files, false, None));
        samples.push(t.elapsed());
    }
    (count, median_ms(samples))
}

fn main() {
    let sym = Name::method(format!("App\\{OWNER}"), HOT_METHOD);
    let reps = 3usize;

    println!(
        "=== REACHABILITY PRE-FILTER: cold `{OWNER}::{HOT_METHOD}` references ===\n\
         (1 in {REACH_EVERY} files reference the owner; the rest share the method name)\n"
    );
    println!(
        "{:>7}  {:>10} {:>10}  {:>12} {:>12}  {:>8}",
        "files", "before_n", "after_n", "before_ms", "after_ms", "speedup"
    );
    for &n in &[100usize, 500, 1000, 3000] {
        let (before_n, before_ms) =
            cold_ms(n, reps, &sym, |s, _| s.workspace_file_paths());
        let (after_n, after_ms) = cold_ms(n, reps, &sym, |s, reach| {
            s.workspace_file_paths()
                .into_iter()
                .filter(|u| reach.contains(u.as_ref()))
                .collect()
        });
        println!(
            "{n:>7}  {before_n:>10} {after_n:>10}  {before_ms:>12.3} {after_ms:>12.3}  {:>7.2}x",
            before_ms / after_ms
        );
    }

    println!("\n=== WARM SWEEP: background warm_analysis_sweep, then first query ===");
    println!(
        "{:>7}  {:>10} {:>12} {:>12} {:>10}",
        "files", "sweep_ms", "cold_ms", "warmed_ms", "noop_ms"
    );
    let mut warm_gate_ok = true;
    let mut noop_by_n: Vec<(usize, f64)> = Vec::new();
    for &n in &[500usize, 1000, 3000] {
        // Cold: first query pays per-candidate analyze_file.
        let (store, reachable) = build(n);
        let files: Vec<Arc<str>> = store
            .workspace_file_paths()
            .into_iter()
            .filter(|u| reachable.contains(u.as_ref()))
            .collect();
        let t = Instant::now();
        std::hint::black_box(store.indexed_references(&sym, &files, false, None));
        let cold_ms = t.elapsed().as_secs_f64() * 1000.0;

        // Warmed: the sweep runs in the background after indexing; the first
        // user-visible query is then a memo hit.
        let (store, reachable) = build(n);
        let files: Vec<Arc<str>> = store
            .workspace_file_paths()
            .into_iter()
            .filter(|u| reachable.contains(u.as_ref()))
            .collect();
        let t = Instant::now();
        let cancel = store.begin_warm_sweep();
        store.warm_analysis_sweep(&[], &cancel);
        let sweep_ms = t.elapsed().as_secs_f64() * 1000.0;
        let t = Instant::now();
        std::hint::black_box(store.indexed_references(&sym, &files, false, None));
        let warmed_ms = t.elapsed().as_secs_f64() * 1000.0;
        // No-op re-sweep: nothing changed, so this is pure memo validation.
        let t = Instant::now();
        let cancel = store.begin_warm_sweep();
        store.warm_analysis_sweep(&[], &cancel);
        let noop_ms = t.elapsed().as_secs_f64() * 1000.0;
        noop_by_n.push((n, noop_ms));
        println!("{n:>7}  {sweep_ms:>10.1} {cold_ms:>12.3} {warmed_ms:>12.3} {noop_ms:>10.3}");
        // CI gate: a warmed first query regressing toward cold cost means the
        // sweep no longer populates the memos the read path consumes.
        if warmed_ms >= cold_ms * 0.10 {
            eprintln!("GATE: warmed {warmed_ms:.3} ms >= 10% of cold {cold_ms:.3} ms at N={n}");
            warm_gate_ok = false;
        }
    }
    // CI gate: a no-op re-sweep must stay cheap and ~linear in file count —
    // anything superlinear (or absolutely expensive) means revalidation has
    // turned back into re-analysis and the edit-idle re-warm will burn CPU.
    let noop_small = noop_by_n.first().map(|&(_, ms)| ms).unwrap_or(0.0);
    let noop_large = noop_by_n.last().map(|&(_, ms)| ms).unwrap_or(0.0);
    const NOOP_CEILING_MS: f64 = 250.0;
    const NOOP_SLOPE_MAX: f64 = 10.0;
    let noop_ok = noop_large < NOOP_CEILING_MS
        && (noop_small <= 0.5 || noop_large / noop_small <= NOOP_SLOPE_MAX);
    if !noop_ok {
        eprintln!(
            "GATE: no-op re-sweep {noop_large:.3} ms at N=3000 (ceiling {NOOP_CEILING_MS}), \
             slope vs N=500 {:.1}x (max {NOOP_SLOPE_MAX})",
            noop_large / noop_small
        );
    }

    println!("\n=== SESSION AXIS: repeated references after unrelated edits (N=1000) ===");
    let (store, reachable) = build(1000);
    let after: Vec<Arc<str>> = store
        .workspace_file_paths()
        .into_iter()
        .filter(|u| reachable.contains(u.as_ref()))
        .collect();
    for _ in 0..3 {
        std::hint::black_box(store.indexed_references(&sym, &after, false, None));
    }
    println!("{:>5}  {:>10}  {:>13}", "iter", "edited", "references_ms");
    let mut session = Vec::new();
    for iter in 0..12usize {
        let victim = (iter * 7) % 999;
        let (u, t) = noise_file(victim);
        let _ = t;
        store.ingest(
            u,
            &format!(
                "<?php\nnamespace App;\nclass N{victim} {{ public function {HOT_METHOD}(): void {{}}\n\
                 public function run(): void {{ $this->{HOT_METHOD}(); /* edit {iter} */ }} }}\n"
            ),
        );
        let t0 = Instant::now();
        std::hint::black_box(store.indexed_references(&sym, &after, false, None));
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        session.push(ms);
        println!("{iter:>5}  {:>10}  {ms:>13.3}", format!("N{victim}"));
    }
    let (early, late) = (session[1], *session.last().unwrap());
    println!(
        "late/early = {:.2}x  →  {}",
        late / early,
        if late / early >= 1.5 {
            "DEGRADES"
        } else {
            "FLAT"
        }
    );

    let session_ok = long_session_gate(&sym);

    // Boot-to-warm at 10k files: informational, dashboard-tracked. The plan's
    // decision point for disk-persisting analysis results is this number
    // exceeding ~30 s on real hardware — synthetic files are cheaper than real
    // code (~4x, measured on the Laravel fixture), so scale accordingly.
    {
        let (store, _) = build(10_000);
        let t = Instant::now();
        let cancel = store.begin_warm_sweep();
        store.warm_analysis_sweep(&[], &cancel);
        println!(
            "\n=== BOOT-TO-WARM: full sweep at N=10000 = {:.1} s ===",
            t.elapsed().as_secs_f64()
        );
    }

    if !warm_gate_ok || !noop_ok || !session_ok {
        std::process::exit(1);
    }
}

/// The degradation scenario that matters most: a long session of hub-file
/// edits, each followed by a background re-warm and a references query. Query
/// latency at iteration 50 must match iteration 1 — any upward trend means
/// state accumulates somewhere on the edit→re-warm→query cycle.
fn long_session_gate(sym: &Name) -> bool {
    const N: usize = 1000;
    const ITERS: usize = 50;
    const WINDOW: usize = 10;
    const RATIO_MAX: f64 = 1.5;

    let (store, reachable) = build(N);
    let files: Vec<Arc<str>> = store
        .workspace_file_paths()
        .into_iter()
        .filter(|u| reachable.contains(u.as_ref()))
        .collect();
    let cancel = store.begin_warm_sweep();
    store.warm_analysis_sweep(&[], &cancel);

    let (svc_url, _) = service_file();
    let mut queries: Vec<f64> = Vec::with_capacity(ITERS);
    println!("\n=== LONG SESSION GATE: {ITERS}x (hub edit -> re-warm -> query) at N={N} ===");
    for iter in 0..ITERS {
        // Edit the owner class itself — every candidate depends on it, so this
        // is the worst-case per-edit invalidation.
        store.ingest(
            svc_url.clone(),
            &format!(
                "<?php\nnamespace App;\nclass {OWNER} {{\n    public function {HOT_METHOD}(): void {{ /* edit {iter} */ }}\n}}\n"
            ),
        );
        let cancel = store.begin_warm_sweep();
        store.warm_analysis_sweep(&[], &cancel);
        let t = Instant::now();
        std::hint::black_box(store.indexed_references(sym, &files, false, None));
        queries.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    let median = |s: &[f64]| -> f64 {
        let mut v = s.to_vec();
        v.sort_by(|a, b| a.total_cmp(b));
        v[v.len() / 2]
    };
    let early = median(&queries[..WINDOW]);
    let late = median(&queries[ITERS - WINDOW..]);
    let ratio = late / early;
    let ok = ratio < RATIO_MAX;
    println!(
        "query median: first {WINDOW} = {early:.3} ms, last {WINDOW} = {late:.3} ms, \
         late/early = {ratio:.2}x (max {RATIO_MAX}) → {}",
        if ok { "FLAT" } else { "DEGRADES" }
    );
    ok
}
