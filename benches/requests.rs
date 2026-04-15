use std::sync::Arc;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use tower_lsp::lsp_types::{Position, Url};

use php_lsp::ast::ParsedDoc;
use php_lsp::completion::{CompletionCtx, filtered_completions_at};
use php_lsp::definition::goto_definition;
use php_lsp::hover::hover_info;

const MEDIUM: &str = include_str!("fixtures/medium_class.php");
const SMALL: &str = include_str!("fixtures/small_class.php");
const CONTROLLER: &str = include_str!("fixtures/controller.php");
const SERVICE: &str = include_str!("fixtures/service.php");
const REPOSITORY: &str = include_str!("fixtures/repository.php");
const EVENTS: &str = include_str!("fixtures/events.php");
const VALIDATOR: &str = include_str!("fixtures/validator.php");

// medium_class.php — line 97, char 20: `getTitle` method declaration
const POS_METHOD: Position = Position {
    line: 97,
    character: 20,
};
// medium_class.php — line 95, char 16: `Article` property block
const POS_MEMBER: Position = Position {
    line: 95,
    character: 16,
};

// controller.php — line 17, char 14: `UserService` property type (defined in service.php)
const POS_SERVICE_TYPE: Position = Position {
    line: 17,
    character: 14,
};
// controller.php — line 25, char 35: `UserService` constructor param (defined in service.php)
const POS_SERVICE_CTOR: Position = Position {
    line: 25,
    character: 35,
};
// controller.php — line 38, char 24: `->` arrow trigger on `$this->service->`
const POS_ARROW: Position = Position {
    line: 38,
    character: 24,
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

    let mut group = c.benchmark_group("hover");
    group.bench_function("single_method", |b| {
        b.iter(|| hover_info(MEDIUM, &medium_doc, POS_METHOD, &[]));
    });
    group.bench_function("single_member", |b| {
        b.iter(|| hover_info(MEDIUM, &medium_doc, POS_MEMBER, &[]));
    });
    group.bench_function("cross_file_service_type", |b| {
        b.iter(|| hover_info(CONTROLLER, &ctrl_doc, POS_SERVICE_TYPE, &other_docs));
    });
    group.bench_function("cross_file_ctor_param", |b| {
        b.iter(|| hover_info(CONTROLLER, &ctrl_doc, POS_SERVICE_CTOR, &other_docs));
    });
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
        b.iter(|| goto_definition(&medium_uri, MEDIUM, &medium_doc, &[], POS_METHOD));
    });
    group.bench_function("cross_file_service_type", |b| {
        b.iter(|| {
            goto_definition(
                &ctrl_uri,
                CONTROLLER,
                &ctrl_doc,
                &other_docs,
                POS_SERVICE_TYPE,
            )
        });
    });
    group.bench_function("cross_file_ctor_param", |b| {
        b.iter(|| {
            goto_definition(
                &ctrl_uri,
                CONTROLLER,
                &ctrl_doc,
                &other_docs,
                POS_SERVICE_CTOR,
            )
        });
    });
    for &n in &[1usize, 5, 10] {
        group.bench_with_input(BenchmarkId::new("scale", n), &ten_docs[..n], |b, docs| {
            b.iter(|| goto_definition(&ctrl_uri, CONTROLLER, &ctrl_doc, docs, POS_SERVICE_TYPE));
        });
    }
    group.finish();
}

fn bench_completion(c: &mut Criterion) {
    let ctrl_doc = Arc::new(ParsedDoc::parse(CONTROLLER.to_owned()));
    let other_parsed: Vec<Arc<ParsedDoc>> = cross_file_docs().into_iter().map(|(_, p)| p).collect();

    let ctx = CompletionCtx {
        source: Some(CONTROLLER),
        position: Some(POS_ARROW),
        meta: None,
        doc_uri: None,
        file_imports: None,
    };

    c.bench_function("completion/cross_file_arrow", |b| {
        b.iter(|| filtered_completions_at(&ctrl_doc, &other_parsed, Some(">"), &ctx));
    });
}

criterion_group!(benches, bench_hover, bench_definition, bench_completion);
criterion_main!(benches);
