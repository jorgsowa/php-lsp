use std::sync::Arc;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use tower_lsp::lsp_types::{Position, Url};

use php_lsp::ast::ParsedDoc;
use php_lsp::completion::{CompletionCtx, filtered_completions_at};
use php_lsp::definition::goto_definition;
use php_lsp::hover::hover_info;

// ---------------------------------------------------------------------------
// Fixture sources
// ---------------------------------------------------------------------------

const MEDIUM: &str = include_str!("fixtures/medium_class.php");
const SMALL: &str = include_str!("fixtures/small_class.php");
const CONTROLLER: &str = include_str!("fixtures/controller.php");
const SERVICE: &str = include_str!("fixtures/service.php");
const REPOSITORY: &str = include_str!("fixtures/repository.php");
const EVENTS: &str = include_str!("fixtures/events.php");
const VALIDATOR: &str = include_str!("fixtures/validator.php");

// ---------------------------------------------------------------------------
// Cursor positions — single-file (medium_class.php)
// ---------------------------------------------------------------------------

/// Line 97 (0-indexed), char 20 — lands on `getTitle` method declaration.
const POS_MEDIUM_METHOD: Position = Position {
    line: 97,
    character: 20,
};

/// Line 95 (0-indexed), char 16 — lands inside the `Article` property block.
const POS_MEDIUM_MEMBER: Position = Position {
    line: 95,
    character: 16,
};

// ---------------------------------------------------------------------------
// Cursor positions — cross-file (controller.php)
// ---------------------------------------------------------------------------

/// Line 17 (0-indexed), char 14 — lands on `UserService` type hint in:
///   `    private UserService $service;`
/// `UserService` is defined in service.php (cross-file resolution).
const POS_CTRL_SERVICE_TYPE: Position = Position {
    line: 17,
    character: 14,
};

/// Line 25 (0-indexed), char 35 — lands on `UserService` in the constructor
/// parameter:  `…__construct(UserService $service, …)`
/// Resolves to service.php.
const POS_CTRL_CTOR_SERVICE: Position = Position {
    line: 25,
    character: 35,
};

/// Line 38 (0-indexed), char 24 — lands on `service` in:
///   `        return $this->service->listAll();`
/// After `->` triggers member-completion against UserService from service.php.
const POS_CTRL_ARROW: Position = Position {
    line: 38,
    character: 24,
};

// ---------------------------------------------------------------------------
// Helper: build the five-file cross-file context
// ---------------------------------------------------------------------------

type OtherDocs = Vec<(Url, Arc<ParsedDoc>)>;

fn cross_file_docs() -> OtherDocs {
    vec![
        (
            Url::parse("file:///bench/service.php").unwrap(),
            Arc::new(ParsedDoc::parse(SERVICE.to_owned())),
        ),
        (
            Url::parse("file:///bench/repository.php").unwrap(),
            Arc::new(ParsedDoc::parse(REPOSITORY.to_owned())),
        ),
        (
            Url::parse("file:///bench/events.php").unwrap(),
            Arc::new(ParsedDoc::parse(EVENTS.to_owned())),
        ),
        (
            Url::parse("file:///bench/validator.php").unwrap(),
            Arc::new(ParsedDoc::parse(VALIDATOR.to_owned())),
        ),
        (
            Url::parse("file:///bench/small_class.php").unwrap(),
            Arc::new(ParsedDoc::parse(SMALL.to_owned())),
        ),
    ]
}

// ---------------------------------------------------------------------------
// Hover benchmarks
// ---------------------------------------------------------------------------

fn bench_hover_single(c: &mut Criterion) {
    let doc = Arc::new(ParsedDoc::parse(MEDIUM.to_owned()));
    // Baseline: no cross-file context — measures pure single-file hover cost.
    let other_docs: OtherDocs = vec![];

    c.bench_function("hover/single_method", |b| {
        b.iter(|| hover_info(MEDIUM, &doc, POS_MEDIUM_METHOD, &other_docs));
    });

    c.bench_function("hover/single_member", |b| {
        b.iter(|| hover_info(MEDIUM, &doc, POS_MEDIUM_MEMBER, &other_docs));
    });
}

fn bench_hover_cross_file(c: &mut Criterion) {
    let doc = Arc::new(ParsedDoc::parse(CONTROLLER.to_owned()));
    // Build context once outside the measured loop.
    let other_docs = cross_file_docs();

    // Hover over `UserService` property type — resolves to service.php.
    c.bench_function("hover/cross_file_service_type", |b| {
        b.iter(|| hover_info(CONTROLLER, &doc, POS_CTRL_SERVICE_TYPE, &other_docs));
    });

    // Hover over constructor parameter type `UserService`.
    c.bench_function("hover/cross_file_ctor_param", |b| {
        b.iter(|| hover_info(CONTROLLER, &doc, POS_CTRL_CTOR_SERVICE, &other_docs));
    });
}

// ---------------------------------------------------------------------------
// Goto-definition benchmarks
// ---------------------------------------------------------------------------

fn bench_definition_single(c: &mut Criterion) {
    let doc = Arc::new(ParsedDoc::parse(MEDIUM.to_owned()));
    let uri = Url::parse("file:///bench/medium.php").unwrap();
    let other_docs: OtherDocs = vec![];

    // Definition of `getTitle` — same file, no cross-file lookup needed.
    c.bench_function("definition/single_method", |b| {
        b.iter(|| goto_definition(&uri, MEDIUM, &doc, &other_docs, POS_MEDIUM_METHOD));
    });
}

fn bench_definition_cross_file(c: &mut Criterion) {
    let doc = Arc::new(ParsedDoc::parse(CONTROLLER.to_owned()));
    let uri = Url::parse("file:///bench/controller.php").unwrap();
    // Build once outside the measured loop.
    let other_docs = cross_file_docs();

    // `UserService` on the property declaration line — must walk other_docs to find it.
    c.bench_function("definition/cross_file_service_type", |b| {
        b.iter(|| goto_definition(&uri, CONTROLLER, &doc, &other_docs, POS_CTRL_SERVICE_TYPE));
    });

    // `UserService` in the constructor parameter list.
    c.bench_function("definition/cross_file_ctor_param", |b| {
        b.iter(|| goto_definition(&uri, CONTROLLER, &doc, &other_docs, POS_CTRL_CTOR_SERVICE));
    });
}

fn bench_definition_scale(c: &mut Criterion) {
    // Show how goto_definition scales with 1 / 5 / 10 files in other_docs.
    let doc = Arc::new(ParsedDoc::parse(CONTROLLER.to_owned()));
    let uri = Url::parse("file:///bench/controller.php").unwrap();

    let all_extra: OtherDocs = cross_file_docs();
    // Build a 10-file context by repeating entries with distinct URIs.
    let ten_docs: OtherDocs = (0..10)
        .map(|i| {
            let (_, parsed) = &all_extra[i % all_extra.len()];
            let url = Url::parse(&format!("file:///bench/extra_{}.php", i)).unwrap();
            (url, Arc::clone(parsed))
        })
        .collect();

    let mut group = c.benchmark_group("definition_scale");

    for &n in &[1usize, 5, 10] {
        let docs: OtherDocs = ten_docs[..n].to_vec();
        group.bench_with_input(BenchmarkId::from_parameter(n), &docs, |b, d| {
            b.iter(|| goto_definition(&uri, CONTROLLER, &doc, d, POS_CTRL_SERVICE_TYPE));
        });
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// Completion benchmarks
// ---------------------------------------------------------------------------

fn bench_completion_cross_file(c: &mut Criterion) {
    let doc = Arc::new(ParsedDoc::parse(CONTROLLER.to_owned()));
    let other_docs = cross_file_docs();
    // Strip Url/Arc wrappers — filtered_completions_at wants &[Arc<ParsedDoc>].
    let other_parsed: Vec<Arc<ParsedDoc>> = other_docs.into_iter().map(|(_, p)| p).collect();

    // Trigger `->` completion at `$this->service->` (the `>` is the trigger
    // character).  Position lands just after the second `->` so member
    // completions for UserService are expected.
    let ctx = CompletionCtx {
        source: Some(CONTROLLER),
        position: Some(POS_CTRL_ARROW),
        meta: None,
        doc_uri: None,
        file_imports: None,
    };

    c.bench_function("completion/cross_file_arrow", |b| {
        b.iter(|| filtered_completions_at(&doc, &other_parsed, Some(">"), &ctx));
    });
}

// ---------------------------------------------------------------------------
// Legacy / backwards-compat benchmarks kept for comparison
// ---------------------------------------------------------------------------

fn bench_hover_legacy(c: &mut Criterion) {
    let doc = Arc::new(ParsedDoc::parse(MEDIUM.to_owned()));
    let other_doc = Arc::new(ParsedDoc::parse(SMALL.to_owned()));
    let other_uri = Url::parse("file:///bench/small.php").unwrap();
    let other_docs = vec![(other_uri, other_doc)];

    c.bench_function("requests/hover_method", |b| {
        b.iter(|| hover_info(MEDIUM, &doc, POS_MEDIUM_METHOD, &other_docs));
    });

    c.bench_function("requests/hover_member_access", |b| {
        b.iter(|| hover_info(MEDIUM, &doc, POS_MEDIUM_MEMBER, &other_docs));
    });
}

fn bench_definition_legacy(c: &mut Criterion) {
    let doc = Arc::new(ParsedDoc::parse(MEDIUM.to_owned()));
    let other_doc = Arc::new(ParsedDoc::parse(SMALL.to_owned()));
    let uri = Url::parse("file:///bench/medium.php").unwrap();
    let other_uri = Url::parse("file:///bench/small.php").unwrap();
    let other_docs = vec![(other_uri, other_doc)];

    c.bench_function("requests/goto_definition_method", |b| {
        b.iter(|| goto_definition(&uri, MEDIUM, &doc, &other_docs, POS_MEDIUM_METHOD));
    });
}

fn bench_completion_legacy(c: &mut Criterion) {
    let doc = Arc::new(ParsedDoc::parse(MEDIUM.to_owned()));
    let other_doc = Arc::new(ParsedDoc::parse(SMALL.to_owned()));
    let other_docs_arc: Vec<Arc<ParsedDoc>> = vec![other_doc];

    let ctx = CompletionCtx {
        source: Some(MEDIUM),
        position: Some(POS_MEDIUM_METHOD),
        meta: None,
        doc_uri: None,
        file_imports: None,
    };

    c.bench_function("requests/completion_no_trigger", |b| {
        b.iter(|| filtered_completions_at(&doc, &other_docs_arc, None, &ctx));
    });
}

criterion_group!(
    benches,
    bench_hover_single,
    bench_hover_cross_file,
    bench_definition_single,
    bench_definition_cross_file,
    bench_definition_scale,
    bench_completion_cross_file,
    bench_hover_legacy,
    bench_definition_legacy,
    bench_completion_legacy,
);
criterion_main!(benches);
