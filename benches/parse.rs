use criterion::{Criterion, criterion_group, criterion_main};
use php_lsp::ast::ParsedDoc;

const SMALL: &str = include_str!("fixtures/small_class.php");
const MEDIUM: &str = include_str!("fixtures/medium_class.php");
const LARGE_IFACE: &str = include_str!("fixtures/interface_large.php");

fn bench_parse_small(c: &mut Criterion) {
    c.bench_function("parse/small_class", |b| {
        b.iter(|| ParsedDoc::parse(SMALL.to_owned()))
    });
}

fn bench_parse_medium(c: &mut Criterion) {
    c.bench_function("parse/medium_class", |b| {
        b.iter(|| ParsedDoc::parse(MEDIUM.to_owned()))
    });
}

fn bench_parse_interface_large(c: &mut Criterion) {
    c.bench_function("parse/interface_large", |b| {
        b.iter(|| ParsedDoc::parse(LARGE_IFACE.to_owned()))
    });
}

criterion_group!(
    benches,
    bench_parse_small,
    bench_parse_medium,
    bench_parse_interface_large
);
criterion_main!(benches);
