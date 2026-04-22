use std::sync::Arc;

use iai_callgrind::{black_box, library_benchmark, library_benchmark_group, main};
use tower_lsp::lsp_types::Url;

use php_lsp::ast::{MethodReturnsMap, ParsedDoc};
use php_lsp::document_store::DocumentStore;
use php_lsp::hover::hover_info;
use php_lsp::type_map::build_method_returns;

const MEDIUM: &str = include_str!("fixtures/medium_class.php");
const SMALL: &str = include_str!("fixtures/small_class.php");
const SERVICE: &str = include_str!("fixtures/service.php");
const REPOSITORY: &str = include_str!("fixtures/repository.php");

// --- parse ---

#[library_benchmark]
fn parse_medium() -> ParsedDoc {
    black_box(ParsedDoc::parse(MEDIUM.to_owned()))
}

library_benchmark_group!(name = parse_group; benchmarks = parse_medium);

// --- index ---

fn setup_store_50() -> DocumentStore {
    let store = DocumentStore::new();
    let fixtures = [SMALL, MEDIUM, SERVICE, REPOSITORY];
    for i in 0..50usize {
        let uri = Url::parse(&format!("file:///iai/file{i}.php")).unwrap();
        store.index(uri, fixtures[i % fixtures.len()]);
    }
    store
}

#[library_benchmark]
#[bench::fifty_files(setup_store_50())]
fn index_get_all_docs(store: DocumentStore) {
    black_box(store.all_docs());
}

library_benchmark_group!(name = index_group; benchmarks = index_get_all_docs);

// --- hover ---

type HoverSetup = (
    Arc<ParsedDoc>,
    Arc<MethodReturnsMap>,
    Vec<(Url, Arc<ParsedDoc>, Arc<MethodReturnsMap>)>,
);

fn setup_hover() -> HoverSetup {
    let doc = Arc::new(ParsedDoc::parse(MEDIUM.to_owned()));
    let doc_mr = Arc::new(build_method_returns(&doc));
    let other = [SERVICE, REPOSITORY]
        .iter()
        .enumerate()
        .map(|(i, src)| {
            let url = Url::parse(&format!("file:///iai/other{i}.php")).unwrap();
            let parsed = Arc::new(ParsedDoc::parse((*src).to_owned()));
            let mr = Arc::new(build_method_returns(&parsed));
            (url, parsed, mr)
        })
        .collect();
    (doc, doc_mr, other)
}

#[library_benchmark]
#[bench::method_position(setup_hover())]
fn hover_cross_file((doc, doc_mr, others): HoverSetup) {
    // Line 109, char 19 — on `getTitle` method
    let pos = tower_lsp::lsp_types::Position {
        line: 109,
        character: 19,
    };
    black_box(hover_info(MEDIUM, &doc, &doc_mr, pos, &others));
}

library_benchmark_group!(name = hover_group; benchmarks = hover_cross_file);

main!(
    library_benchmark_groups = parse_group,
    index_group,
    hover_group
);
