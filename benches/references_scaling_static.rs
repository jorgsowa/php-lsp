//! Sibling of `references_scaling.rs` for **static** members.
//!
//! `references_scaling.rs` measures an intentionally *unimplemented* upper
//! bound: an AND-gate (member name AND owner class name) rejected as unsound
//! for instance members (a receiver typed by an inherited property, or a
//! factory return, never names the owner class in the calling file). Static
//! members don't have that failure mode — every real static call site
//! (`Owner::m()`, `self::`/`static::`/`parent::m()` from inside the
//! hierarchy, an aliased `use Owner as X; X::m()`) textually names either the
//! owner or one of its subtypes. mir's `reference_gate_strategy` (mir 0.63+)
//! implements exactly this AND-gate for a resolved static-only member, using
//! the transitive subtype closure as the owner-group.
//!
//! Unlike `references_scaling.rs`, this bench calls the real, unmodified
//! `docs.indexed_references(...)` end-to-end — no hand-filtered candidate
//! list — since the speedup now lives inside mir's own gate. Its value is a
//! regression gate going forward: cold latency at a realistic N must stay
//! well below what an ungated OR-scan over the same fixture would cost.
//!
//! Run: `cargo bench --bench references_scaling_static`

use std::sync::Arc;
use std::time::{Duration, Instant};

use mir_analyzer::Name;
use php_lsp::document_store::DocumentStore;
use tower_lsp::lsp_types::Url;

const HOT_METHOD: &str = "process";
const OWNER: &str = "Service";
/// 1 in N files actually calls `App\Service::process()` (directly or via a
/// subclass); the rest define/call their own unrelated static `process()`.
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

/// Calls `App\Service::process()` statically — never via a subclass, never
/// via self/static/parent — the plainest reachable shape.
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

/// Noise: defines and calls its *own* static `process()` — text-matches the
/// method name but never names `Service`, so it cannot resolve to
/// `Service::process`.
fn noise_file(i: usize) -> (Url, String) {
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

fn build(n: usize) -> DocumentStore {
    let store = DocumentStore::new();
    let (su, st) = service_file();
    store.ingest(su, &st);
    for i in 0..n.saturating_sub(1) {
        let (u, t) = if i % REACH_EVERY == 0 {
            reachable_file(i)
        } else {
            noise_file(i)
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
fn cold_ms(n: usize, reps: usize, sym: &Name) -> (usize, f64) {
    let mut samples = Vec::with_capacity(reps);
    let mut count = 0;
    for _ in 0..reps {
        let store = build(n);
        let files: Vec<Arc<str>> = store.workspace_file_paths();
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

    println!("=== STATIC-MEMBER AND-GATE: cold `{OWNER}::{HOT_METHOD}` references ===");
    println!(
        "(1 in {REACH_EVERY} files call the owner statically; the rest share the method name \
         on unrelated classes)\n"
    );
    println!("{:>7}  {:>10}  {:>10}", "files", "candidates", "cold_ms");
    // Generous absolute ceiling, not a synthetic before/after ratio: the
    // real gate now runs unconditionally inside `indexed_references`, so
    // there's no in-process "before" branch to compare against. This is a
    // regression gate — cold latency must stay roughly flat as N grows,
    // not climb the way an ungated OR-scan (paying full `analyze_file` on
    // every noise file) would.
    const COLD_MS_CEILING: f64 = 500.0;
    let mut gate_ok = true;
    for &n in &[100usize, 500, 1000, 3000] {
        let (count, ms) = cold_ms(n, reps, &sym);
        println!("{n:>7}  {count:>10}  {ms:>10.3}");
        if ms >= COLD_MS_CEILING {
            eprintln!("GATE: cold {ms:.3} ms at N={n} >= ceiling {COLD_MS_CEILING}");
            gate_ok = false;
        }
    }

    if !gate_ok {
        std::process::exit(1);
    }
}
