use std::sync::Arc;

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use tower_lsp::lsp_types::{Position, Url};

use php_lsp::ast::{MethodReturnsMap, ParsedDoc};
use php_lsp::call_hierarchy::{incoming_calls, outgoing_calls, prepare_call_hierarchy};
use php_lsp::completion::{CompletionCtx, filtered_completions_at};
use php_lsp::definition::goto_definition;
use php_lsp::file_index::FileIndex;
use php_lsp::hover::hover_info;
use php_lsp::implementation::find_implementations;
use php_lsp::references::{SymbolKind, find_references};
use php_lsp::rename::rename;
use php_lsp::symbols::{document_symbols, workspace_symbols_from_index};
use php_lsp::type_map::build_method_returns;

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
type HoverDocs = Vec<(Url, Arc<ParsedDoc>, Arc<MethodReturnsMap>)>;

fn to_hover_docs(docs: &OtherDocs) -> HoverDocs {
    docs.iter()
        .map(|(u, d)| {
            let mr = Arc::new(build_method_returns(d));
            (u.clone(), Arc::clone(d), mr)
        })
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
    let medium_mr = build_method_returns(&medium_doc);
    let ctrl_doc = Arc::new(ParsedDoc::parse(CONTROLLER.to_owned()));
    let ctrl_mr = build_method_returns(&ctrl_doc);
    let other_docs = cross_file_docs();
    let hover_others = to_hover_docs(&other_docs);

    // Build a 10-entry context by cycling the 5 cross-file docs (mirrors the
    // definition scale benchmark so the two are directly comparable).
    let ten_hover_docs: HoverDocs = (0..10)
        .map(|i| {
            let (_, parsed, mr) = &hover_others[i % hover_others.len()];
            let url = Url::parse(&format!("file:///bench/extra_{i}.php")).unwrap();
            (url, Arc::clone(parsed), Arc::clone(mr))
        })
        .collect();

    let mut group = c.benchmark_group("hover");
    group.bench_function("single_method", |b| {
        b.iter(|| black_box(hover_info(MEDIUM, &medium_doc, &medium_mr, POS_METHOD, &[])));
    });
    group.bench_function("single_member", |b| {
        b.iter(|| black_box(hover_info(MEDIUM, &medium_doc, &medium_mr, POS_MEMBER, &[])));
    });
    group.bench_function("cross_file_service_type", |b| {
        b.iter(|| {
            black_box(hover_info(
                CONTROLLER,
                &ctrl_doc,
                &ctrl_mr,
                POS_SERVICE_TYPE,
                &hover_others,
            ))
        });
    });
    group.bench_function("cross_file_ctor_param", |b| {
        b.iter(|| {
            black_box(hover_info(
                CONTROLLER,
                &ctrl_doc,
                &ctrl_mr,
                POS_SERVICE_CTOR,
                &hover_others,
            ))
        });
    });
    for &n in &[1usize, 5, 10] {
        group.bench_with_input(
            BenchmarkId::new("scale", n),
            &ten_hover_docs[..n],
            |b, docs| {
                b.iter(|| {
                    black_box(hover_info(
                        CONTROLLER,
                        &ctrl_doc,
                        &ctrl_mr,
                        POS_SERVICE_TYPE,
                        docs,
                    ))
                });
            },
        );
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
        meta: None,
        doc_uri: None,
        file_imports: None,
        doc_returns: None,
        other_returns: None,
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

fn bench_references(c: &mut Criterion) {
    let other_docs = cross_file_docs();

    // Build a 10-entry context by cycling the 5 cross-file docs.
    let ten_docs: OtherDocs = (0..10)
        .map(|i| {
            let (_, parsed) = &other_docs[i % other_docs.len()];
            let url = Url::parse(&format!("file:///bench/extra_{i}.php")).unwrap();
            (url, Arc::clone(parsed))
        })
        .collect();

    let mut group = c.benchmark_group("references");

    // Single-file: search for `getTitle` (a method defined and called in medium_class.php).
    let medium_uri = Url::parse("file:///bench/medium.php").unwrap();
    let medium_doc = Arc::new(ParsedDoc::parse(MEDIUM.to_owned()));
    let single_doc = vec![(medium_uri, medium_doc)];
    group.bench_function("single_file_method", |b| {
        b.iter(|| {
            black_box(find_references(
                "getTitle",
                &single_doc,
                false,
                Some(SymbolKind::Method),
            ))
        });
    });

    // Cross-file: search for `UserService` (a class referenced across controller + service).
    group.bench_function("cross_file_class", |b| {
        b.iter(|| {
            black_box(find_references(
                "UserService",
                &other_docs,
                false,
                Some(SymbolKind::Class),
            ))
        });
    });

    // Scale: same query over 1 / 5 / 10 files.
    for &n in &[1usize, 5, 10] {
        group.bench_with_input(BenchmarkId::new("scale", n), &ten_docs[..n], |b, docs| {
            b.iter(|| {
                black_box(find_references(
                    "UserService",
                    docs,
                    false,
                    Some(SymbolKind::Class),
                ))
            });
        });
    }

    group.finish();
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
        .filter(|e| e.path().extension().map_or(false, |x| x == "php"))
        .filter_map(|e| {
            let url = Url::from_file_path(e.path()).ok()?;
            let src = std::fs::read_to_string(e.path()).ok()?;
            Some((url, Arc::new(ParsedDoc::parse(src))))
        })
        .collect();
    Some(docs)
}

fn bench_references_laravel(c: &mut Criterion) {
    let Some(docs) = laravel_docs() else {
        eprintln!(
            "Laravel fixture not found — run `scripts/setup_laravel_fixture.sh` to enable references/laravel_framework"
        );
        return;
    };
    eprintln!("Laravel fixture: {} PHP files (references)", docs.len());

    let mut group = c.benchmark_group("references");
    group.sample_size(10);
    // `Str` is widely referenced across Illuminate — a realistic hot symbol.
    group.bench_function("laravel_framework", |b| {
        b.iter(|| {
            black_box(find_references(
                "Str",
                &docs,
                false,
                Some(SymbolKind::Class),
            ))
        });
    });
    // Method-kind query on a public method of an open (non-final) hierarchy:
    // `save` on `Illuminate\Database\Eloquent\Model`. The mir codebase fast path
    // does not apply here (public + non-final), so this exercises the AST walker
    // + substring pre-filter path.
    group.bench_function("laravel_framework_method_save", |b| {
        b.iter(|| {
            black_box(find_references(
                "save",
                &docs,
                false,
                Some(SymbolKind::Method),
            ))
        });
    });
    group.finish();
}

fn bench_completion_laravel(c: &mut Criterion) {
    let Some(docs) = laravel_docs() else {
        eprintln!(
            "Laravel fixture not found — run `scripts/setup_laravel_fixture.sh` to enable completion/laravel_framework"
        );
        return;
    };
    // Derive a parsed-only view for the completion API.
    let other_parsed: Vec<Arc<ParsedDoc>> = docs.iter().map(|(_, p)| Arc::clone(p)).collect();

    let ctrl_doc = Arc::new(ParsedDoc::parse(CONTROLLER.to_owned()));
    let ctx = CompletionCtx {
        source: Some(CONTROLLER),
        position: Some(POS_ARROW),
        meta: None,
        doc_uri: None,
        file_imports: None,
        doc_returns: None,
        other_returns: None,
    };

    let mut group = c.benchmark_group("completion");
    group.sample_size(10);
    group.bench_function("laravel_framework", |b| {
        b.iter(|| {
            black_box(filtered_completions_at(
                &ctrl_doc,
                &other_parsed,
                Some(">"),
                &ctx,
            ))
        });
    });
    group.finish();
}

fn bench_rename(c: &mut Criterion) {
    let other_docs = cross_file_docs();

    let mut group = c.benchmark_group("rename");
    // Cross-file: rename UserService → UserServiceRenamed across the small
    // fixture set (controller + service + repository + …).
    group.bench_function("cross_file_class", |b| {
        b.iter(|| black_box(rename("UserService", "UserServiceRenamed", &other_docs)));
    });

    if let Some(docs) = laravel_docs() {
        eprintln!("Laravel fixture: {} PHP files (rename)", docs.len());
        group.sample_size(10);
        group.bench_function("laravel_framework", |b| {
            b.iter(|| black_box(rename("Str", "StrRenamed", &docs)));
        });
    } else {
        eprintln!("Laravel fixture not found — skipping rename/laravel_framework");
    }
    group.finish();
}

fn to_indexes(docs: &OtherDocs) -> Vec<(Url, Arc<FileIndex>)> {
    docs.iter()
        .map(|(uri, parsed)| (uri.clone(), Arc::new(FileIndex::extract(parsed))))
        .collect()
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

fn bench_implementation(c: &mut Criterion) {
    let other_docs = cross_file_docs();

    let mut group = c.benchmark_group("implementation");
    // Cross-file: find classes that extend / implement `UserService`.
    group.bench_function("cross_file_class", |b| {
        b.iter(|| black_box(find_implementations("UserService", None, &other_docs)));
    });

    if let Some(docs) = laravel_docs() {
        eprintln!("Laravel fixture: {} PHP files (implementation)", docs.len());
        group.sample_size(10);
        // A widely-implemented Illuminate contract.
        group.bench_function("laravel_framework", |b| {
            b.iter(|| black_box(find_implementations("Arrayable", None, &docs)));
        });
    } else {
        eprintln!("Laravel fixture not found — skipping implementation/laravel_framework");
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
    let other_docs = cross_file_docs();

    // Prepare the item once — it's part of the call-hierarchy request but the
    // interesting cost is incoming/outgoing, which dominates in real usage.
    let item_service = prepare_call_hierarchy("UserService", &other_docs);

    let mut group = c.benchmark_group("call_hierarchy");
    group.bench_function("prepare/cross_file", |b| {
        b.iter(|| black_box(prepare_call_hierarchy("UserService", &other_docs)));
    });
    if let Some(ref item) = item_service {
        group.bench_function("incoming/cross_file", |b| {
            b.iter(|| black_box(incoming_calls(item, &other_docs)));
        });
        group.bench_function("outgoing/cross_file", |b| {
            b.iter(|| black_box(outgoing_calls(item, &other_docs)));
        });
    }

    if let Some(docs) = laravel_docs() {
        eprintln!("Laravel fixture: {} PHP files (call_hierarchy)", docs.len());
        group.sample_size(10);
        group.bench_function("prepare/laravel_framework", |b| {
            b.iter(|| black_box(prepare_call_hierarchy("Str", &docs)));
        });
        if let Some(item) = prepare_call_hierarchy("Str", &docs) {
            group.bench_function("incoming/laravel_framework", |b| {
                b.iter(|| black_box(incoming_calls(&item, &docs)));
            });
            group.bench_function("outgoing/laravel_framework", |b| {
                b.iter(|| black_box(outgoing_calls(&item, &docs)));
            });
        }
    } else {
        eprintln!("Laravel fixture not found — skipping call_hierarchy/laravel_framework");
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_hover,
    bench_definition,
    bench_completion,
    bench_references,
    bench_references_laravel,
    bench_completion_laravel,
    bench_rename,
    bench_workspace_symbol,
    bench_implementation,
    bench_document_symbol,
    bench_call_hierarchy
);
criterion_main!(benches);
