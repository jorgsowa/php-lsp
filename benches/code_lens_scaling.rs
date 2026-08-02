//! Code-lens reachability-scan batching benchmark.
//!
//! Every reference-count lens whose symbol needs FQN/text-needle narrowing
//! (a class reference, or a public static method/constructor) used to call
//! `DocumentStore::reference_candidate_files` independently, each paying its
//! own full pass over the workspace. `code_lenses` now collects every such
//! declaration in one AST walk and resolves all of their scopes together via
//! `DocumentStore::batch_reference_candidate_files`, sharing one Aho-Corasick
//! pass over the workspace instead of one per declaration.
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

use php_lsp::analysis::code_lens::code_lenses;
use php_lsp::ast::ParsedDoc;
use php_lsp::document_store::DocumentStore;
use tower_lsp_server::ls_types::Uri;

const OWNER: &str = "Service";

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

fn code_lens_ms(n_noise: usize, num_methods: usize, reps: usize) -> (usize, f64) {
    let mut samples = Vec::with_capacity(reps);
    let mut lens_count = 0;
    for _ in 0..reps {
        let (store, doc, url) = build(n_noise, num_methods);
        let imports = HashMap::new();
        let t = Instant::now();
        let lenses =
            std::hint::black_box(code_lenses(&url, &doc, &store, &imports, None, || false));
        samples.push(t.elapsed());
        lens_count = lenses.len();
    }
    (lens_count, median_ms(samples))
}

fn main() {
    let reps = 5;
    println!(
        "=== CODE LENS: N narrowing-eligible declarations (1 class + N-1 public static methods) ==="
    );
    println!(
        "{:>7}  {:>8}  {:>10}  {:>10}",
        "files", "lenses", "cold_ms", "ms/lens"
    );
    for &n_noise in &[500usize, 1500, 3000] {
        for &num_methods in &[10usize, 50, 100] {
            let (lens_count, ms) = code_lens_ms(n_noise, num_methods, reps);
            println!(
                "{n_noise:>7}  {lens_count:>8}  {ms:>10.3}  {:>10.4}",
                ms / lens_count as f64
            );
        }
    }
}
