use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use php_lsp::ast::ParsedDoc;

const FIXTURES: &[(&str, &str)] = &[
    ("small_class", include_str!("fixtures/small_class.php")),
    ("medium_class", include_str!("fixtures/medium_class.php")),
    (
        "interface_large",
        include_str!("fixtures/interface_large.php"),
    ),
];

fn bench_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("parse");
    for (name, src) in FIXTURES {
        group.bench_with_input(BenchmarkId::from_parameter(name), src, |b, s| {
            b.iter(|| ParsedDoc::parse((*s).to_owned()));
        });
    }
    group.finish();
}

criterion_group!(benches, bench_parse);
criterion_main!(benches);
