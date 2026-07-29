use std::sync::Arc;

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use rayon::prelude::*;
use tower_lsp::lsp_types::{Position, Url};

use php_lsp::ast::ParsedDoc;
use php_lsp::call_hierarchy::{outgoing_calls_indexed, prepare_call_hierarchy_indexed};
use php_lsp::completion::{CompletionCtx, filtered_completions_at};
use php_lsp::definition::goto_definition;
use php_lsp::file_index::FileIndex;
use php_lsp::hover::hover_info_with_maps;
use php_lsp::symbol_map::SymbolMap;
use php_lsp::symbols::{document_symbols, workspace_symbols_from_index};

const MEDIUM: &str = include_str!("fixtures/medium_class.php");
const SMALL: &str = include_str!("fixtures/small_class.php");
const CONTROLLER: &str = include_str!("fixtures/controller.php");
const SERVICE: &str = include_str!("fixtures/service.php");
const REPOSITORY: &str = include_str!("fixtures/repository.php");
const EVENTS: &str = include_str!("fixtures/events.php");
const VALIDATOR: &str = include_str!("fixtures/validator.php");

// medium_class.php — LSP line 109 (file line 110), char 19: on `getTitle` in
//   `    public function getTitle(): string`
const POS_METHOD: Position = Position {
    line: 109,
    character: 19,
};
// medium_class.php — LSP line 94 (file line 95), char 20: on `title` in
//   `   private string $title;`
const POS_MEMBER: Position = Position {
    line: 94,
    character: 20,
};

// controller.php — LSP line 17 (file line 18), char 13: on `UserService` in
//   `    private UserService $service;`
const POS_SERVICE_TYPE: Position = Position {
    line: 17,
    character: 13,
};
// controller.php — LSP line 25 (file line 26), char 32: on `UserService` in
//   `    public function __construct(UserService $service, …)`
const POS_SERVICE_CTOR: Position = Position {
    line: 25,
    character: 32,
};
// controller.php — LSP line 38 (file line 39), char 31: after `->` in
//   `        return $this->service->listAll();`
const POS_ARROW: Position = Position {
    line: 38,
    character: 31,
};

type OtherDocs = Vec<(Url, Arc<ParsedDoc>)>;
type SymbolMaps = Vec<(Url, Arc<SymbolMap>)>;

fn to_symbol_maps(docs: &OtherDocs) -> SymbolMaps {
    docs.iter()
        .map(|(u, d)| (u.clone(), Arc::new(SymbolMap::build(d))))
        .collect()
}

fn cross_file_docs() -> OtherDocs {
    [
        ("file:///bench/service.php", SERVICE),
        ("file:///bench/repository.php", REPOSITORY),
        ("file:///bench/events.php", EVENTS),
        ("file:///bench/validator.php", VALIDATOR),
        ("file:///bench/small_class.php", SMALL),
    ]
    .into_iter()
    .map(|(url, src)| {
        (
            Url::parse(url).unwrap(),
            Arc::new(ParsedDoc::parse(src.to_owned())),
        )
    })
    .collect()
}

fn bench_hover(c: &mut Criterion) {
    let medium_doc = Arc::new(ParsedDoc::parse(MEDIUM.to_owned()));
    let ctrl_doc = Arc::new(ParsedDoc::parse(CONTROLLER.to_owned()));
    let other_docs = cross_file_docs();

    // Pre-computed symbol maps — simulates what salsa caches between requests.
    let other_maps = to_symbol_maps(&other_docs);

    // Build 10-entry contexts (5 docs cycled twice) for scale benchmarks.
    let ten_docs: OtherDocs = (0..10)
        .map(|i| {
            let (_, parsed) = &other_docs[i % other_docs.len()];
            let url = Url::parse(&format!("file:///bench/extra_{i}.php")).unwrap();
            (url, Arc::clone(parsed))
        })
        .collect();
    let ten_maps: SymbolMaps = ten_docs
        .iter()
        .map(|(u, d)| (u.clone(), Arc::new(SymbolMap::build(d))))
        .collect();

    let mut group = c.benchmark_group("hover");
    group.bench_function("single_method", |b| {
        b.iter(|| {
            black_box(hover_info_with_maps(
                MEDIUM,
                &medium_doc,
                None,
                POS_METHOD,
                &[],
                &[],
                None,
                None,
            ))
        });
    });
    group.bench_function("single_member", |b| {
        b.iter(|| {
            black_box(hover_info_with_maps(
                MEDIUM,
                &medium_doc,
                None,
                POS_MEMBER,
                &[],
                &[],
                None,
                None,
            ))
        });
    });

    // Cross-file with precomputed symbol maps (O(1) lookup).
    group.bench_function("cross_file/service_type", |b| {
        b.iter(|| {
            black_box(hover_info_with_maps(
                CONTROLLER,
                &ctrl_doc,
                None,
                POS_SERVICE_TYPE,
                &other_docs,
                &other_maps,
                None,
                None,
            ))
        });
    });
    group.bench_function("cross_file/ctor_param", |b| {
        b.iter(|| {
            black_box(hover_info_with_maps(
                CONTROLLER,
                &ctrl_doc,
                None,
                POS_SERVICE_CTOR,
                &other_docs,
                &other_maps,
                None,
                None,
            ))
        });
    });

    // Scale: 1 / 5 / 10 other files.
    for &n in &[1usize, 5, 10] {
        group.bench_with_input(BenchmarkId::new("scale", n), &ten_maps[..n], |b, maps| {
            b.iter(|| {
                black_box(hover_info_with_maps(
                    CONTROLLER,
                    &ctrl_doc,
                    None,
                    POS_SERVICE_TYPE,
                    &[],
                    maps,
                    None,
                    None,
                ))
            });
        });
    }
    group.finish();
}

fn bench_definition(c: &mut Criterion) {
    let medium_doc = Arc::new(ParsedDoc::parse(MEDIUM.to_owned()));
    let medium_uri = Url::parse("file:///bench/medium.php").unwrap();
    let ctrl_doc = Arc::new(ParsedDoc::parse(CONTROLLER.to_owned()));
    let ctrl_uri = Url::parse("file:///bench/controller.php").unwrap();
    let other_docs = cross_file_docs();

    // Build a 10-entry context for the scale benchmark by cycling the 5 cross-file docs.
    let ten_docs: OtherDocs = (0..10)
        .map(|i| {
            let (_, parsed) = &other_docs[i % other_docs.len()];
            let url = Url::parse(&format!("file:///bench/extra_{i}.php")).unwrap();
            (url, Arc::clone(parsed))
        })
        .collect();

    let mut group = c.benchmark_group("definition");
    group.bench_function("single_method", |b| {
        b.iter(|| {
            black_box(goto_definition(
                &medium_uri,
                MEDIUM,
                &medium_doc,
                &[],
                POS_METHOD,
            ))
        });
    });
    group.bench_function("cross_file_service_type", |b| {
        b.iter(|| {
            black_box(goto_definition(
                &ctrl_uri,
                CONTROLLER,
                &ctrl_doc,
                &other_docs,
                POS_SERVICE_TYPE,
            ))
        });
    });
    group.bench_function("cross_file_ctor_param", |b| {
        b.iter(|| {
            black_box(goto_definition(
                &ctrl_uri,
                CONTROLLER,
                &ctrl_doc,
                &other_docs,
                POS_SERVICE_CTOR,
            ))
        });
    });
    for &n in &[1usize, 5, 10] {
        group.bench_with_input(BenchmarkId::new("scale", n), &ten_docs[..n], |b, docs| {
            b.iter(|| {
                black_box(goto_definition(
                    &ctrl_uri,
                    CONTROLLER,
                    &ctrl_doc,
                    docs,
                    POS_SERVICE_TYPE,
                ))
            });
        });
    }
    group.finish();
}

fn bench_completion(c: &mut Criterion) {
    let ctrl_doc = Arc::new(ParsedDoc::parse(CONTROLLER.to_owned()));
    // Derive parsed-only docs from cross_file_docs to avoid double-parsing.
    let other_parsed: Vec<Arc<ParsedDoc>> = cross_file_docs().into_iter().map(|(_, p)| p).collect();

    let ctx = CompletionCtx {
        source: Some(CONTROLLER),
        position: Some(POS_ARROW),
        doc_uri: None,
        file_imports: None,
        find_class_doc: None,
        workspace_class_search: None,
        analysis: None,
        session: None,
        laravel: None,
    };

    c.bench_function("completion/cross_file_arrow", |b| {
        b.iter(|| {
            black_box(filtered_completions_at(
                &ctrl_doc,
                &other_parsed,
                Some(">"),
                &ctx,
            ))
        });
    });
}

// ── Laravel-scale benches ─────────────────────────────────────────────────────
//
// These load the Laravel fixture (via `scripts/setup_laravel_fixture.sh`) and
// measure each request against ~1,600 parsed files. They auto-skip when the
// fixture is absent.

fn laravel_docs() -> Option<OtherDocs> {
    let fixture_dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/fixtures/laravel/src");
    if !fixture_dir.exists() {
        return None;
    }
    let docs: OtherDocs = walkdir::WalkDir::new(&fixture_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "php"))
        .filter_map(|e| {
            let url = Url::from_file_path(e.path()).ok()?;
            let src = std::fs::read_to_string(e.path()).ok()?;
            Some((url, Arc::new(ParsedDoc::parse(src))))
        })
        .collect();
    Some(docs)
}

fn bench_completion_laravel(c: &mut Criterion) {
    let Some(docs) = laravel_docs() else {
        eprintln!(
            "Laravel fixture not found — run `scripts/setup_laravel_fixture.sh` to enable completion/laravel_framework"
        );
        return;
    };
    eprintln!("Laravel fixture: {} PHP files (completion)", docs.len());
    // Derive a parsed-only view for the completion API.
    let other_parsed: Vec<Arc<ParsedDoc>> = docs.iter().map(|(_, p)| Arc::clone(p)).collect();

    // Build a class-name → doc map once so the indexed benchmark can use
    // O(1) lookup instead of the O(n) linear scan. Mirrors what the backend
    // does via get_workspace_index_salsa() + get_doc_salsa().
    let class_doc_map: std::collections::HashMap<String, Arc<ParsedDoc>> = {
        let mut map = std::collections::HashMap::new();
        for (_, doc) in &docs {
            let idx = FileIndex::extract(doc);
            for cls in idx.classes {
                map.entry(cls.name.to_string())
                    .or_insert_with(|| Arc::clone(doc));
            }
        }
        map
    };

    let ctrl_doc = Arc::new(ParsedDoc::parse(CONTROLLER.to_owned()));

    let mut group = c.benchmark_group("completion");
    group.sample_size(10);

    // Baseline: linear scan through all docs per class in the hierarchy.
    let ctx_linear = CompletionCtx {
        source: Some(CONTROLLER),
        position: Some(POS_ARROW),
        doc_uri: None,
        file_imports: None,
        find_class_doc: None,
        workspace_class_search: None,
        analysis: None,
        session: None,
        laravel: None,
    };
    group.bench_function("laravel_framework", |b| {
        b.iter(|| {
            black_box(filtered_completions_at(
                &ctrl_doc,
                &other_parsed,
                Some(">"),
                &ctx_linear,
            ))
        });
    });

    // Fast path: O(1) workspace-index lookup per class.
    let find_fn = |name: &str| -> Option<Arc<ParsedDoc>> { class_doc_map.get(name).cloned() };
    let ctx_indexed = CompletionCtx {
        source: Some(CONTROLLER),
        position: Some(POS_ARROW),
        doc_uri: None,
        file_imports: None,
        find_class_doc: Some(&find_fn),
        workspace_class_search: None,
        analysis: None,
        session: None,
        laravel: None,
    };
    group.bench_function("laravel_framework_indexed", |b| {
        b.iter(|| {
            black_box(filtered_completions_at(
                &ctrl_doc,
                &other_parsed,
                Some(">"),
                &ctx_indexed,
            ))
        });
    });

    // Realistic case: complete on Illuminate\Database\Eloquent\Builder.
    const BUILDER_SRC: &str =
        "<?php\nuse Illuminate\\Database\\Eloquent\\Builder;\n$b = new Builder();\n$b->";
    let builder_doc = Arc::new(ParsedDoc::parse(BUILDER_SRC.to_owned()));
    let builder_pos = tower_lsp::lsp_types::Position {
        line: 3,
        character: 4,
    };

    let ctx_builder_linear = CompletionCtx {
        source: Some(BUILDER_SRC),
        position: Some(builder_pos),
        doc_uri: None,
        file_imports: None,
        find_class_doc: None,
        workspace_class_search: None,
        analysis: None,
        session: None,
        laravel: None,
    };
    group.bench_function("laravel_builder_linear", |b| {
        b.iter(|| {
            black_box(filtered_completions_at(
                &builder_doc,
                &other_parsed,
                Some(">"),
                &ctx_builder_linear,
            ))
        });
    });

    let ctx_builder_indexed = CompletionCtx {
        source: Some(BUILDER_SRC),
        position: Some(builder_pos),
        doc_uri: None,
        file_imports: None,
        find_class_doc: Some(&find_fn),
        workspace_class_search: None,
        analysis: None,
        session: None,
        laravel: None,
    };
    group.bench_function("laravel_builder_indexed", |b| {
        b.iter(|| {
            black_box(filtered_completions_at(
                &builder_doc,
                &other_parsed,
                Some(">"),
                &ctx_builder_indexed,
            ))
        });
    });

    group.finish();
}

fn to_indexes(docs: &OtherDocs) -> Vec<(Url, Arc<FileIndex>)> {
    docs.iter()
        .map(|(uri, parsed)| (uri.clone(), Arc::new(FileIndex::extract(parsed))))
        .collect()
}

fn bench_semantic_tokens(c: &mut Criterion) {
    use php_lsp::analysis::semantic_tokens::{semantic_tokens, semantic_tokens_range};

    let mut group = c.benchmark_group("semantic_tokens");
    let medium = Arc::new(ParsedDoc::parse(MEDIUM.to_owned()));
    group.bench_function("full_medium", |b| {
        b.iter(|| black_box(semantic_tokens(MEDIUM, &medium)));
    });

    if let Some(docs) = laravel_docs() {
        // Largest file in the fixture — worst case for full-document requests
        // and for the collect-then-filter cost of range requests.
        let (_, big) = docs
            .iter()
            .max_by_key(|(_, d)| d.source().len())
            .expect("laravel fixture is non-empty");
        eprintln!(
            "semantic_tokens largest laravel file: {} bytes",
            big.source().len()
        );
        group.sample_size(20);
        group.bench_function("full_laravel_largest", |b| {
            b.iter(|| black_box(semantic_tokens(big.source(), big)));
        });
        // Viewport-sized range request: editors ask for ~50-100 lines.
        let viewport = tower_lsp::lsp_types::Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 50,
                character: 0,
            },
        };
        group.bench_function("range_viewport_laravel_largest", |b| {
            b.iter(|| black_box(semantic_tokens_range(big.source(), big, viewport)));
        });
    } else {
        eprintln!("Laravel fixture not found — skipping semantic_tokens/laravel benches");
    }
    group.finish();
}

fn bench_inlay_hints(c: &mut Criterion) {
    use php_lsp::analysis::inlay_hints::inlay_hints;

    let full_range = tower_lsp::lsp_types::Range {
        start: Position {
            line: 0,
            character: 0,
        },
        end: Position {
            line: u32::MAX,
            character: 0,
        },
    };

    let mut group = c.benchmark_group("inlay_hints");
    let medium = Arc::new(ParsedDoc::parse(MEDIUM.to_owned()));
    group.bench_function("medium_no_workspace", |b| {
        b.iter(|| black_box(inlay_hints(MEDIUM, &medium, None, full_range, &[])));
    });

    if let Some(docs) = laravel_docs() {
        let indexes = to_indexes(&docs);
        let ctrl = Arc::new(ParsedDoc::parse(CONTROLLER.to_owned()));
        group.sample_size(20);
        group.bench_function("controller_laravel_workspace", |b| {
            b.iter(|| black_box(inlay_hints(CONTROLLER, &ctrl, None, full_range, &indexes)));
        });
    } else {
        eprintln!("Laravel fixture not found — skipping inlay_hints/laravel bench");
    }
    group.finish();
}

fn bench_definition_index_fallback(c: &mut Criterion) {
    let Some(docs) = laravel_docs() else {
        eprintln!("Laravel fixture not found — skipping definition/index_fallback_*");
        return;
    };
    eprintln!(
        "Laravel fixture: {} PHP files (definition fallback)",
        docs.len()
    );
    let indexes = to_indexes(&docs);
    let wi = php_lsp::db::workspace_index::WorkspaceIndexData::from_files(indexes.clone());

    let mut group = c.benchmark_group("definition");
    group.sample_size(10);
    // Worst case for the linear scan: a name that matches nothing forces a
    // full walk over every declaration in every FileIndex.
    group.bench_function("index_fallback_linear_miss", |b| {
        b.iter(|| {
            black_box(php_lsp::definition::find_declaration_in_indexes(
                "zzz_no_such_symbol",
                &indexes,
            ))
        });
    });
    group.bench_function("index_fallback_map_miss", |b| {
        b.iter(|| black_box(wi.find_declaration("zzz_no_such_symbol", None)));
    });
    // Typical hit: a method name defined deep in the framework.
    group.bench_function("index_fallback_linear_hit", |b| {
        b.iter(|| {
            black_box(php_lsp::definition::find_declaration_in_indexes(
                "firstOrCreate",
                &indexes,
            ))
        });
    });
    group.bench_function("index_fallback_map_hit", |b| {
        b.iter(|| black_box(wi.find_declaration("firstOrCreate", None)));
    });
    group.finish();
}

fn bench_workspace_symbol(c: &mut Criterion) {
    let other_docs = cross_file_docs();
    let other_indexes = to_indexes(&other_docs);

    let mut group = c.benchmark_group("workspace_symbol");
    // Small-set fuzzy search: query matches `UserService`, `UserRepository`, etc.
    group.bench_function("fuzzy_small", |b| {
        b.iter(|| black_box(workspace_symbols_from_index("User", &other_indexes)));
    });

    if let Some(docs) = laravel_docs() {
        eprintln!(
            "Laravel fixture: {} PHP files (workspace_symbol)",
            docs.len()
        );
        let indexes = to_indexes(&docs);
        group.sample_size(10);
        // Common prefix across Illuminate — should match many symbols.
        group.bench_function("laravel_framework", |b| {
            b.iter(|| black_box(workspace_symbols_from_index("Str", &indexes)));
        });
    } else {
        eprintln!("Laravel fixture not found — skipping workspace_symbol/laravel_framework");
    }
    group.finish();
}

fn bench_document_symbol(c: &mut Criterion) {
    let medium_doc = Arc::new(ParsedDoc::parse(MEDIUM.to_owned()));
    let ctrl_doc = Arc::new(ParsedDoc::parse(CONTROLLER.to_owned()));

    let mut group = c.benchmark_group("document_symbol");
    group.bench_function("medium_class", |b| {
        b.iter(|| black_box(document_symbols(MEDIUM, &medium_doc)));
    });
    group.bench_function("controller", |b| {
        b.iter(|| black_box(document_symbols(CONTROLLER, &ctrl_doc)));
    });
    group.finish();
}

fn bench_call_hierarchy(c: &mut Criterion) {
    let mut group = c.benchmark_group("call_hierarchy");

    if let Some(docs) = laravel_docs() {
        eprintln!("Laravel fixture: {} PHP files (call_hierarchy)", docs.len());
        group.sample_size(10);

        // Indexed variants: decls_by_name lookups instead of per-callee
        // workspace scans — the path the server handlers use.
        let wi = php_lsp::db::workspace_index::WorkspaceIndexData::from_files(to_indexes(&docs));
        let doc_map: std::collections::HashMap<Url, Arc<ParsedDoc>> =
            docs.iter().cloned().collect();
        let get_doc = |u: &Url| doc_map.get(u).cloned();
        group.bench_function("prepare_indexed/laravel_framework", |b| {
            b.iter(|| black_box(prepare_call_hierarchy_indexed("camel", &wi, &get_doc)));
        });
        // `Str` is a class name; prepare only yields items for functions and
        // methods, so the outgoing bench needs a method symbol.
        let method_item = prepare_call_hierarchy_indexed("camel", &wi, &get_doc);
        assert!(
            method_item.is_some(),
            "expected `camel` (Str::camel) to resolve in the Laravel fixture"
        );
        if let Some(item) = method_item {
            group.bench_function("outgoing_indexed/laravel_framework", |b| {
                b.iter(|| black_box(outgoing_calls_indexed(&item, &wi, &get_doc)));
            });
        }
    } else {
        eprintln!("Laravel fixture not found — skipping call_hierarchy/laravel_framework");
    }
    group.finish();
}

/// Load raw PHP source strings from the Laravel fixture without pre-parsing them.
/// Used by `bench_workspace_parse` to measure parse + index cost in isolation.
fn laravel_sources() -> Option<Vec<(Url, String)>> {
    let fixture_dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/fixtures/laravel/src");
    if !fixture_dir.exists() {
        return None;
    }
    let sources: Vec<(Url, String)> = walkdir::WalkDir::new(&fixture_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "php"))
        .filter_map(|e| {
            let url = Url::from_file_path(e.path()).ok()?;
            let src = std::fs::read_to_string(e.path()).ok()?;
            Some((url, src))
        })
        .collect();
    Some(sources)
}

fn bench_workspace_parse(c: &mut Criterion) {
    let Some(sources) = laravel_sources() else {
        eprintln!(
            "Laravel fixture not found — run `scripts/setup_laravel_fixture.sh` to enable workspace_parse benchmarks"
        );
        return;
    };

    eprintln!(
        "Laravel fixture: {} PHP files (workspace_parse)",
        sources.len()
    );

    let mut group = c.benchmark_group("workspace_parse");
    group.sample_size(10);

    group.bench_function("sequential", |b| {
        b.iter(|| {
            sources.iter().for_each(|(_, src)| {
                let doc = ParsedDoc::parse(src.clone());
                std::hint::black_box(php_lsp::file_index::FileIndex::extract(&doc));
            });
        });
    });

    group.bench_function("rayon", |b| {
        b.iter(|| {
            sources.par_iter().for_each(|(_, src)| {
                let doc = ParsedDoc::parse(src.clone());
                std::hint::black_box(php_lsp::file_index::FileIndex::extract(&doc));
            });
        });
    });

    group.finish();
}

fn bench_symbol_map(c: &mut Criterion) {
    let other_docs = cross_file_docs();
    let medium_doc = Arc::new(ParsedDoc::parse(MEDIUM.to_owned()));

    let mut group = c.benchmark_group("symbol_map");

    // Build cost (one-time per file, amortized over subsequent lookups).
    group.bench_function("build/small_class", |b| {
        let doc = Arc::new(ParsedDoc::parse(SMALL.to_owned()));
        b.iter(|| black_box(SymbolMap::build(&doc)));
    });
    group.bench_function("build/medium_class", |b| {
        b.iter(|| black_box(SymbolMap::build(&medium_doc)));
    });
    for (name, src) in &[
        ("service", SERVICE),
        ("repository", REPOSITORY),
        ("events", EVENTS),
        ("validator", VALIDATOR),
    ] {
        let doc = Arc::new(ParsedDoc::parse((*src).to_owned()));
        group.bench_function(format!("build/{name}"), |b| {
            b.iter(|| black_box(SymbolMap::build(&doc)));
        });
    }

    // Lookup cost on a pre-built map (shows per-request savings).
    let medium_map = Arc::new(SymbolMap::build(&medium_doc));
    group.bench_function("lookup_hit/medium_class", |b| {
        b.iter(|| black_box(medium_map.lookup("getTitle", |_| true)));
    });
    group.bench_function("lookup_miss/medium_class", |b| {
        b.iter(|| black_box(medium_map.lookup("nonexistent_symbol_xyz", |_| true)));
    });

    // Build cost on all 5 cross-file fixtures (baseline for setup cost).
    group.bench_function("build/all_5_fixtures", |b| {
        b.iter(|| {
            for (_, doc) in &other_docs {
                black_box(SymbolMap::build(doc));
            }
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_hover,
    bench_definition,
    bench_definition_index_fallback,
    bench_semantic_tokens,
    bench_inlay_hints,
    bench_completion,
    bench_completion_laravel,
    bench_workspace_symbol,
    bench_document_symbol,
    bench_call_hierarchy,
    bench_workspace_parse,
    bench_symbol_map
);
criterion_main!(benches);
