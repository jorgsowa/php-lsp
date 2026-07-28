//! Find-references read-path performance guard.
//!
//! The references read path is a memoized `analyze_file` query over the
//! candidate set — no ingest, no shared-index mutation. This bench pins two
//! properties on the Laravel fixture:
//!
//! - per-request time stays FLAT as the background-warmed file count grows
//!   (re-introducing per-request mutation would make it climb again); and
//! - visibility scoping (a private method's references live only in its
//!   declaring file) collapses the candidate set from every text-match to one
//!   file.
//!
//! Auto-skips when the Laravel fixture is absent.

use std::sync::Arc;
use std::time::{Duration, Instant};

use mir_analyzer::{AnalysisSession, Name, PhpVersion};
use php_lsp::document_store::DocumentStore;
use tower_lsp::lsp_types::Url;

const METHOD: &str = "save";
const CANDIDATE_CAP: usize = 30;
const ITERS_PER_LEVEL: usize = 12;
const WARMUP_ITERS: usize = 4;

struct SourceFile {
    file: Arc<str>,
    text: Arc<str>,
}

fn laravel_sources() -> Option<Vec<SourceFile>> {
    let fixture_dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/fixtures/laravel/src");
    if !fixture_dir.exists() {
        return None;
    }
    let files: Vec<SourceFile> = walkdir::WalkDir::new(&fixture_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "php"))
        .filter_map(|e| {
            let url = Url::from_file_path(e.path()).ok()?;
            let text = std::fs::read_to_string(e.path()).ok()?;
            Some(SourceFile {
                file: Arc::from(url.as_str()),
                text: Arc::from(text.as_str()),
            })
        })
        .collect();
    Some(files)
}

/// The production references read: an `indexed_references_to` posting lookup
/// over the candidate set. Warm files answer from the index; stale ones
/// re-analyze once and recommit.
fn references(session: &AnalysisSession, sym: &Name, files: &[Arc<str>]) {
    std::hint::black_box(session.indexed_references_to(sym, files, false, &|| false));
}

fn mean_ms(samples: &[Duration]) -> f64 {
    let total: Duration = samples.iter().sum();
    total.as_secs_f64() * 1000.0 / samples.len() as f64
}

fn measure(mut op: impl FnMut()) -> f64 {
    let mut samples: Vec<Duration> = Vec::with_capacity(ITERS_PER_LEVEL);
    for iter in 0..ITERS_PER_LEVEL {
        let t0 = Instant::now();
        op();
        let dt = t0.elapsed();
        if iter >= WARMUP_ITERS {
            samples.push(dt);
        }
    }
    mean_ms(&samples)
}

fn main() {
    // The sessions below run mir's parallel analysis before any
    // DocumentStore exists — size the rayon stacks like production first.
    php_lsp::document_store::ensure_rayon_worker_stacks();
    let Some(all) = laravel_sources() else {
        eprintln!(
            "Laravel fixture not found — run scripts/setup_laravel_fixture.sh to enable references_degradation"
        );
        return;
    };

    let candidates: Vec<SourceFile> = all
        .iter()
        .filter(|f| f.text.contains(METHOD))
        .take(CANDIDATE_CAP)
        .map(|f| SourceFile {
            file: f.file.clone(),
            text: f.text.clone(),
        })
        .collect();
    if candidates.is_empty() {
        eprintln!("no candidate files mention `{METHOD}` — cannot run the bench");
        return;
    }
    let candidate_files: Vec<Arc<str>> = candidates.iter().map(|c| c.file.clone()).collect();

    let candidate_keys: std::collections::HashSet<&str> =
        candidates.iter().map(|c| c.file.as_ref()).collect();
    let background: Vec<&SourceFile> = all
        .iter()
        .filter(|f| !candidate_keys.contains(f.file.as_ref()))
        .collect();

    eprintln!(
        "Laravel fixture: {} files; {} candidates mention `{METHOD}`",
        all.len(),
        candidates.len()
    );

    // The codebase key only filters results; the measured cost is `analyze_file`
    // over the candidate set, so any method symbol exercises the path.
    let sym = Name::method("App\\Models\\Model", METHOD);

    // Throwaway run so the first measured level isn't paying first-touch
    // CPU/allocator costs that would skew the cold sample.
    {
        let session = AnalysisSession::new(PhpVersion::LATEST);
        for cand in &candidates {
            session.set_file_text(cand.file.clone(), cand.text.clone());
        }
        for _ in 0..WARMUP_ITERS {
            references(&session, &sym, &candidate_files);
        }
    }

    // Flatness: hold the candidate set fixed, grow the background-warmed file
    // count. Per-request time must not climb with warmed-set size.
    let warm_levels: Vec<usize> = [0usize, 200, 500, 1000, background.len()]
        .into_iter()
        .filter(|&n| n <= background.len())
        .collect();
    println!(
        "\nwarm_files   mean_ms (fixed {}-file references op)",
        candidates.len()
    );
    let mut first = f64::NAN;
    let mut last = f64::NAN;
    for &warm in &warm_levels {
        let session = AnalysisSession::new(PhpVersion::LATEST);
        for sf in background.iter().take(warm) {
            session.set_file_text(sf.file.clone(), sf.text.clone());
        }
        for cand in &candidates {
            session.set_file_text(cand.file.clone(), cand.text.clone());
        }
        let m = measure(|| references(&session, &sym, &candidate_files));
        if first.is_nan() {
            first = m;
        }
        last = m;
        println!("{warm:>10}   {m:>7.3}");
    }
    let ratio = last / first;
    // CI gate: per-request cost climbing with warmed-set size means
    // per-request mutation crept back into the read path. The absolute floor
    // keeps sub-ms timer noise (0.20 → 0.27 ms is a 1.35x) from tripping it.
    let degrades = ratio >= 1.30 && last >= 1.0;
    println!(
        "last/first = {ratio:.2}x (last {last:.3} ms)  → {}",
        if degrades { "DEGRADES" } else { "FLAT" }
    );
    if degrades {
        std::process::exit(1);
    }

    // Visibility scoping: a private method's references can only live in its
    // declaring file, so the handler narrows the candidate set to that one file.
    {
        let session = AnalysisSession::new(PhpVersion::LATEST);
        for sf in &background {
            session.set_file_text(sf.file.clone(), sf.text.clone());
        }
        for cand in &candidates {
            session.set_file_text(cand.file.clone(), cand.text.clone());
        }
        for _ in 0..WARMUP_ITERS {
            references(&session, &sym, &candidate_files);
            references(&session, &sym, &candidate_files[..1]);
        }
        let full = measure(|| references(&session, &sym, &candidate_files));
        let scoped = measure(|| references(&session, &sym, &candidate_files[..1]));
        println!(
            "\nprivate scoping @ full warm: {}-file {full:.3} ms → 1-file {scoped:.3} ms  ({:.0}x)",
            candidates.len(),
            full / scoped,
        );
    }

    let store = DocumentStore::new();
    for f in &all {
        if let Ok(url) = Url::parse(&f.file) {
            store.ingest(url, &f.text);
        }
    }
    scope_narrowing_comparison(&store, all.len());
}

/// Visibility-derived narrowing (improvement #1) vs handing mir the whole
/// workspace, and the memoized subtype graph (#3). For a `private`/`protected`
/// method the handler uses `method_reference_scope` as the candidate set
/// directly, sparing mir even the per-file freshness pass. The scoped lookup
/// is measured across many iterations to confirm it stays cheap and flat —
/// the subtype map is built once inside the memoized `workspace_index`, not
/// rebuilt per request.
fn scope_narrowing_comparison(store: &DocumentStore, total_files: usize) {
    use php_lsp::file_index::{ClassKind, Visibility};

    store.mark_index_ready();
    let ws = store.get_workspace_index_salsa();

    let narrowable = |cls: &php_lsp::file_index::ClassDef| {
        matches!(cls.kind, ClassKind::Class | ClassKind::Enum)
            && cls.traits.is_empty()
            && cls.mixins.is_empty()
    };

    let mut private_target: Option<(String, String)> = None;
    let mut protected_target: Option<(String, String, usize)> = None;
    for (_, idx) in &ws.files {
        for cls in &idx.classes {
            if !narrowable(cls) {
                continue;
            }
            let fqn = cls.fqn.trim_start_matches('\\').to_string();
            if private_target.is_none()
                && let Some(m) = cls
                    .methods
                    .iter()
                    .find(|m| matches!(m.visibility, Visibility::Private))
            {
                private_target = Some((fqn.clone(), m.name.to_string()));
            }
            // Cheap heuristic to pick a demo target that has at least one
            // (plain `extends`) subclass; the scope itself is measured below via
            // method_reference_scope, which goes through mir's resolved graph.
            let sub_count = ws.subtypes_of.get(cls.name.as_ref()).map_or(0, |v| v.len());
            if protected_target.is_none()
                && sub_count > 0
                && let Some(m) = cls
                    .methods
                    .iter()
                    .find(|m| matches!(m.visibility, Visibility::Protected))
            {
                protected_target = Some((fqn, m.name.to_string(), sub_count));
            }
        }
    }

    if let Some((fqn, method)) = private_target {
        let t0 = Instant::now();
        let full = store.workspace_file_paths();
        let full_ms = t0.elapsed().as_secs_f64() * 1000.0;

        let _ = store.method_reference_scope(&fqn, &method);
        let iters = 200;
        let t1 = Instant::now();
        let mut scope_len = 0;
        for _ in 0..iters {
            scope_len = store
                .method_reference_scope(&fqn, &method)
                .map_or(0, |s| s.len());
        }
        let scope_ms = t1.elapsed().as_secs_f64() * 1000.0 / iters as f64;
        println!(
            "\nprivate `{fqn}::{method}` narrowing over {total_files} files:\n  \
             unscoped (workspace_file_paths):                {} files, {full_ms:.3} ms\n  \
             scoped (method_reference_scope, {iters}x mean):   {scope_len} file(s), {scope_ms:.5} ms",
            full.len(),
        );
    } else {
        eprintln!("no narrowable private method found in fixture");
    }

    if let Some((fqn, method, subs)) = protected_target {
        let _ = store.method_reference_scope(&fqn, &method);
        let iters = 200;
        let t = Instant::now();
        let mut scope_len = 0;
        for _ in 0..iters {
            scope_len = store
                .method_reference_scope(&fqn, &method)
                .map_or(0, |s| s.len());
        }
        let ms = t.elapsed().as_secs_f64() * 1000.0 / iters as f64;
        println!(
            "protected `{fqn}::{method}` ({subs} direct subclasses) scoped lookup, \
             {iters}x mean: {scope_len} file(s), {ms:.5} ms (subtype graph memoized in workspace_index)",
        );
    } else {
        eprintln!("no narrowable protected method with subclasses found in fixture");
    }
}
