//! Code-lens reachability-scan batching benchmark.
//!
//! `code_lenses` leaves reference counts for `resolve_code_lens`. The first
//! resolve of a file narrows every lens symbol together via
//! `DocumentStore::batch_reference_candidate_files`, sharing one
//! Aho-Corasick pass over the workspace instead of one per declaration;
//! later resolves reuse it and pay only their own reference lookup.
//!
//! Reports the `codeLens` request alone, resolving the first `VISIBLE`
//! lenses (what an editor viewport asks for), and resolving all of them.
//!
//! Models a class with N public static methods (each narrowing-eligible) in
//! a workspace full of unrelated noise files that share neither the target's
//! namespace nor a `use` import — so the narrowing scan's expensive fallback
//! (a full-text scan) actually has to run per file, rather than being
//! resolved by the cheap namespace/import checks.
//!
//! Run: `cargo bench --bench code_lens_scaling`

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use php_lsp::analysis::code_lens::{CodeLensScopeCache, code_lenses, resolve_code_lens};
use php_lsp::ast::ParsedDoc;
use php_lsp::document_store::DocumentStore;
use tower_lsp_server::ls_types::Uri;

const OWNER: &str = "Service";
const VISIBLE: usize = 30;

fn service_source(num_methods: usize) -> String {
    let mut methods = String::new();
    for m in 0..num_methods {
        methods.push_str(&format!(
            "    public static function alpha{m}(int $x): int {{ return $x + {m}; }}\n"
        ));
    }
    format!("<?php\nnamespace App;\nclass {OWNER} {{\n{methods}}}\n")
}

/// Noise: a different namespace, so the cheap namespace/import checks never
/// resolve it — the narrowing scan's text-mention fallback must run.
fn noise_file(i: usize) -> (Uri, String) {
    let url = format!("file:///synth/N{i}.php").parse::<Uri>().unwrap();
    let text = format!(
        "<?php\nnamespace Other{i};\n\
         class N{i} {{\n\
         \x20   public function run(int $a): int {{\n\
         \x20       return $a + {i};\n\
         \x20   }}\n\
         }}\n"
    );
    (url, text)
}

fn build(n_noise: usize, num_methods: usize) -> (DocumentStore, Arc<ParsedDoc>, Uri) {
    let store = DocumentStore::new();
    let url = ("file:///synth/Service.php").parse::<Uri>().unwrap();
    let src = service_source(num_methods);
    store.ingest(url.clone(), &src);
    for i in 0..n_noise {
        let (u, t) = noise_file(i);
        store.ingest(u, &t);
    }
    store.mark_index_ready();
    let doc = Arc::new(ParsedDoc::parse(src));
    (store, doc, url)
}

fn median_ms(mut s: Vec<Duration>) -> f64 {
    s.sort();
    s[s.len() / 2].as_secs_f64() * 1000.0
}

struct Timings {
    lenses: usize,
    lens_ms: f64,
    visible_ms: f64,
    all_ms: f64,
}

fn code_lens_ms(n_noise: usize, num_methods: usize, reps: usize) -> Timings {
    let mut lens_samples = Vec::with_capacity(reps);
    let mut visible_samples = Vec::with_capacity(reps);
    let mut all_samples = Vec::with_capacity(reps);
    let mut lens_count = 0;
    for _ in 0..reps {
        let (store, doc, url) = build(n_noise, num_methods);
        let imports = HashMap::new();
        let scopes = CodeLensScopeCache::default();
        let rev = store.write_rev();
        let t = Instant::now();
        let lenses = code_lenses(&url, &doc, &store, &imports, &scopes, || false);
        lens_samples.push(t.elapsed());
        lens_count = lenses.len();

        let t = Instant::now();
        for lens in lenses.iter().take(VISIBLE) {
            std::hint::black_box(resolve_code_lens(lens.clone(), &store, &scopes, rev).unwrap());
        }
        visible_samples.push(t.elapsed());

        // Fresh store so "all" doesn't reuse the visible pass's warm caches.
        let (store, doc, url) = build(n_noise, num_methods);
        let rev = store.write_rev();
        let scopes_all = CodeLensScopeCache::default();
        let lenses = code_lenses(&url, &doc, &store, &imports, &scopes_all, || false);
        let t = Instant::now();
        for lens in &lenses {
            std::hint::black_box(
                resolve_code_lens(lens.clone(), &store, &scopes_all, rev).unwrap(),
            );
        }
        all_samples.push(t.elapsed());
    }
    Timings {
        lenses: lens_count,
        lens_ms: median_ms(lens_samples),
        visible_ms: median_ms(visible_samples),
        all_ms: median_ms(all_samples),
    }
}

fn main() {
    let reps = 5;
    println!(
        "=== CODE LENS: N narrowing-eligible declarations (1 class + N-1 public static methods) ==="
    );
    println!(
        "{:>7}  {:>8}  {:>10}  {:>12}  {:>12}",
        "files", "lenses", "lens_ms", "visible_ms", "all_ms"
    );
    for &n_noise in &[500usize, 1500, 3000] {
        for &num_methods in &[10usize, 50, 100] {
            let t = code_lens_ms(n_noise, num_methods, reps);
            println!(
                "{n_noise:>7}  {:>8}  {:>10.3}  {:>12.3}  {:>12.3}",
                t.lenses, t.lens_ms, t.visible_ms, t.all_ms
            );
        }
    }
}
