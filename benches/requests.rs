use std::sync::Arc;

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use tower_lsp::lsp_types::{Position, Url};

use php_lsp::ast::ParsedDoc;
use php_lsp::completion::{CompletionCtx, filtered_completions_at};
use php_lsp::definition::goto_definition;
use php_lsp::hover::hover_info;
use php_lsp::references::{SymbolKind, find_references};

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

    // Build a 10-entry context by cycling the 5 cross-file docs (mirrors the
    // definition scale benchmark so the two are directly comparable).
    let ten_docs: OtherDocs = (0..10)
        .map(|i| {
            let (_, parsed) = &other_docs[i % other_docs.len()];
            let url = Url::parse(&format!("file:///bench/extra_{i}.php")).unwrap();
            (url, Arc::clone(parsed))
        })
        .collect();

    let mut group = c.benchmark_group("hover");
    group.bench_function("single_method", |b| {
        b.iter(|| black_box(hover_info(MEDIUM, &medium_doc, POS_METHOD, &[])));
    });
    group.bench_function("single_member", |b| {
        b.iter(|| black_box(hover_info(MEDIUM, &medium_doc, POS_MEMBER, &[])));
    });
    group.bench_function("cross_file_service_type", |b| {
        b.iter(|| {
            black_box(hover_info(
                CONTROLLER,
                &ctrl_doc,
                POS_SERVICE_TYPE,
                &other_docs,
            ))
        });
    });
    group.bench_function("cross_file_ctor_param", |b| {
        b.iter(|| {
            black_box(hover_info(
                CONTROLLER,
                &ctrl_doc,
                POS_SERVICE_CTOR,
                &other_docs,
            ))
        });
    });
    for &n in &[1usize, 5, 10] {
        group.bench_with_input(BenchmarkId::new("scale", n), &ten_docs[..n], |b, docs| {
            b.iter(|| black_box(hover_info(CONTROLLER, &ctrl_doc, POS_SERVICE_TYPE, docs)));
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
        meta: None,
        doc_uri: None,
        file_imports: None,
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

criterion_group!(
    benches,
    bench_hover,
    bench_definition,
    bench_completion,
    bench_references
);
criterion_main!(benches);
