//! Sibling of `references_scaling.rs` for **static** members.
//!
//! mir 0.63's `reference_gate` admits a cold candidate file for a resolved
//! static method on the *member token alone*. The owner name is deliberately
//! not required: PHP allows static calls through instance receivers
//! (`$obj::m()`) whose file may never name the owner class, so an
//! owner-group AND-gate would silently drop real references. Every recorded
//! static-ref shape (`Owner::m()`, `self::`/`static::`/`parent::m()`,
//! aliased `X::m()`, `$obj::m()`, `'Owner::m'` strings) spells the member,
//! so the member-only gate is exact.
//!
//! Two fixture shapes, both through the real, unmodified
//! `docs.indexed_references(...)`:
//!
//! - DISTINCTIVE MEMBER — noise files mention the owner's short name (a
//!   docblock is enough; the pre-0.63 OR-gate admitted them all) but never
//!   the member token. The member-only gate rejects them without analysis,
//!   so cold latency stays near-flat as N grows. Gated.
//! - GENERIC MEMBER — every noise file text-matches the member name on an
//!   unrelated class. No sound gate can filter these; cold cost is O(N)
//!   full analysis by design. Gated only against catastrophic regression.
//!
//! Run: `cargo bench --bench references_scaling_static`

use std::sync::Arc;
use std::time::{Duration, Instant};

use mir_analyzer::Name;
use php_lsp::document_store::DocumentStore;
use tower_lsp::lsp_types::Url;

const HOT_METHOD: &str = "process";
const OWNER: &str = "Service";
/// 1 in N files actually calls `App\Service::process()`; the rest are noise
/// whose shape depends on the scenario.
const REACH_EVERY: usize = 10;

fn service_file() -> (Url, String) {
    let url = Url::parse("file:///synth/Service.php").unwrap();
    let text = format!(
        "<?php\nnamespace App;\nclass {OWNER} {{\n    public static function {HOT_METHOD}(): void {{}}\n}}\n"
    );
    (url, text)
}

/// Filler so each file is a representative size, not a toy.
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

/// Calls `App\Service::process()` statically — the plainest reachable shape.
fn reachable_file(i: usize) -> (Url, String) {
    let url = Url::parse(&format!("file:///synth/R{i}.php")).unwrap();
    let text = format!(
        "<?php\nnamespace App;\n\
         class R{i} {{\n\
         \x20   public function run(int $a): int {{\n\
         \x20       {OWNER}::{HOT_METHOD}();\n\
         \x20       return $a + {i};\n\
         \x20   }}\n{}}}\n",
        filler()
    );
    (url, text)
}

/// Generic-member noise: defines and calls its *own* static `process()` —
/// text-matches the member token, so the gate must admit it.
fn noise_file_member_match(i: usize) -> (Url, String) {
    let url = Url::parse(&format!("file:///synth/N{i}.php")).unwrap();
    let text = format!(
        "<?php\nnamespace App;\n\
         class N{i} {{\n\
         \x20   public static function {HOT_METHOD}(): void {{}}\n\
         \x20   public function run(int $a): int {{\n\
         \x20       self::{HOT_METHOD}();\n\
         \x20       return $a + {i};\n\
         \x20   }}\n{}}}\n",
        filler()
    );
    (url, text)
}

/// Distinctive-member noise: mentions the owner's short name (docblock —
/// the word-bounded gate scan doesn't care where) but never the member
/// token. The pre-0.63 OR-gate paid a full analysis here; the member-only
/// gate rejects on the text scan alone.
fn noise_file_owner_match(i: usize) -> (Url, String) {
    let url = Url::parse(&format!("file:///synth/N{i}.php")).unwrap();
    let text = format!(
        "<?php\nnamespace App;\n\
         /** Unrelated to the {OWNER} layer. */\n\
         class N{i} {{\n\
         \x20   public function run(int $a): int {{\n\
         \x20       return $a + {i};\n\
         \x20   }}\n{}}}\n",
        filler()
    );
    (url, text)
}

fn build(n: usize, noise: fn(usize) -> (Url, String)) -> DocumentStore {
    let store = DocumentStore::new();
    let (su, st) = service_file();
    store.ingest(su, &st);
    for i in 0..n.saturating_sub(1) {
        let (u, t) = if i % REACH_EVERY == 0 {
            reachable_file(i)
        } else {
            noise(i)
        };
        store.ingest(u, &t);
    }
    store.mark_index_ready();
    store
}

fn median_ms(mut s: Vec<Duration>) -> f64 {
    s.sort();
    s[s.len() / 2].as_secs_f64() * 1000.0
}

/// Median cold latency over the real, unfiltered candidate set: a fresh
/// store per rep so `analyze_file` is never a memo hit.
fn cold_ms(n: usize, reps: usize, sym: &Name, noise: fn(usize) -> (Url, String)) -> (usize, f64) {
    let mut samples = Vec::with_capacity(reps);
    let mut count = 0;
    for _ in 0..reps {
        let store = build(n, noise);
        let files: Vec<Arc<str>> = store.workspace_file_paths().to_vec();
        count = files.len();
        let t = Instant::now();
        std::hint::black_box(store.indexed_references(sym, &files, false, None));
        samples.push(t.elapsed());
    }
    (count, median_ms(samples))
}

fn run_scenario(
    title: &str,
    sym: &Name,
    noise: fn(usize) -> (Url, String),
    ceiling_ms: f64,
) -> bool {
    let reps = 3usize;
    println!("=== {title} ===");
    println!("{:>7}  {:>10}  {:>10}", "files", "candidates", "cold_ms");
    let mut ok = true;
    for &n in &[100usize, 500, 1000, 3000] {
        let (count, ms) = cold_ms(n, reps, sym, noise);
        println!("{n:>7}  {count:>10}  {ms:>10.3}");
        if ms >= ceiling_ms {
            eprintln!("GATE: cold {ms:.3} ms at N={n} >= ceiling {ceiling_ms}");
            ok = false;
        }
    }
    println!();
    ok
}

fn main() {
    let sym = Name::method(format!("App\\{OWNER}"), HOT_METHOD);

    // The shape the member-only gate wins: noise text-matches the owner but
    // not the member, so only the 1-in-10 reachable files pay analysis
    // (~164 ms at N=3000 measured on mir 0.63.0; losing the gate lands at
    // ~775 ms, the ungated number below).
    let distinctive_ok = run_scenario(
        &format!("DISTINCTIVE MEMBER: cold `{OWNER}::{HOT_METHOD}`, noise mentions `{OWNER}` only"),
        &sym,
        noise_file_owner_match,
        400.0,
    );

    // Worst case by design: every file text-matches the member token, so the
    // gate admits all of them and cold cost is O(N) analysis. The generous
    // ceiling only catches order-of-magnitude regressions.
    let generic_ok = run_scenario(
        &format!(
            "GENERIC MEMBER (ungatable worst case): cold `{OWNER}::{HOT_METHOD}`, every file matches `{HOT_METHOD}`"
        ),
        &sym,
        noise_file_member_match,
        2500.0,
    );

    if !distinctive_ok || !generic_ok {
        std::process::exit(1);
    }
}
