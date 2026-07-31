use std::sync::Arc;

use std::hint::black_box;

use iai_callgrind::{library_benchmark, library_benchmark_group, main};
use tower_lsp_server::ls_types::Uri;

use php_lsp::ast::ParsedDoc;
use php_lsp::document_store::DocumentStore;
use php_lsp::hover::hover_info_with_maps;
use php_lsp::symbol_map::SymbolMap;

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

fn setup_store_50() -> (DocumentStore, Vec<Uri>) {
    let store = DocumentStore::new();
    let fixtures = [SMALL, MEDIUM, SERVICE, REPOSITORY];
    let urls: Vec<Uri> = (0..50usize)
        .map(|i| format!("file:///iai/file{i}.php").parse::<Uri>().unwrap())
        .collect();
    for (i, uri) in urls.iter().enumerate() {
        store.ingest(uri.clone(), fixtures[i % fixtures.len()]);
    }
    (store, urls)
}

#[library_benchmark]
#[bench::fifty_files(setup_store_50())]
fn index_get_all_docs(input: (DocumentStore, Vec<Uri>)) {
    let (store, urls) = input;
    black_box(store.docs_for(&urls));
}

library_benchmark_group!(name = index_group; benchmarks = index_get_all_docs);

// --- hover ---

type HoverMapSetup = (
    Arc<ParsedDoc>,
    Vec<(Uri, Arc<ParsedDoc>)>,
    Vec<(Uri, Arc<SymbolMap>)>,
);

fn setup_hover_maps() -> HoverMapSetup {
    let doc = Arc::new(ParsedDoc::parse(MEDIUM.to_owned()));
    let other_docs: Vec<(Uri, Arc<ParsedDoc>)> = [SERVICE, REPOSITORY]
        .iter()
        .enumerate()
        .map(|(i, src)| {
            let url = format!("file:///iai/other{i}.php").parse::<Uri>().unwrap();
            let parsed = Arc::new(ParsedDoc::parse((*src).to_owned()));
            (url, parsed)
        })
        .collect();
    let other_maps: Vec<(Uri, Arc<SymbolMap>)> = other_docs
        .iter()
        .map(|(u, d)| (u.clone(), Arc::new(SymbolMap::build(d))))
        .collect();
    (doc, other_docs, other_maps)
}

#[library_benchmark]
#[bench::method_position(setup_hover_maps())]
fn hover_cross_file_map((doc, other_docs, other_maps): HoverMapSetup) {
    let pos = tower_lsp_server::ls_types::Position {
        line: 109,
        character: 19,
    };
    black_box(hover_info_with_maps(
        MEDIUM,
        &doc,
        None,
        pos,
        &other_docs,
        &other_maps,
        None,
        None,
    ));
}

library_benchmark_group!(
    name = hover_group;
    benchmarks = hover_cross_file_map
);

main!(
    library_benchmark_groups = parse_group,
    index_group,
    hover_group
);
