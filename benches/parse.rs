use criterion::{
    BatchSize, BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main,
};
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
        // Throughput lets Criterion report bytes/sec alongside time, making
        // it easy to spot regressions that are just fixture-size growth.
        group.throughput(Throughput::Bytes(src.len() as u64));
        group.bench_with_input(BenchmarkId::from_parameter(name), src, |b, s| {
            // iter_batched moves the String allocation into the setup phase so
            // only parsing time is measured.  black_box prevents LLVM from
            // eliding the call when the result would otherwise be unused.
            b.iter_batched(
                || (*s).to_owned(),
                |owned| black_box(ParsedDoc::parse(owned)),
                BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

criterion_group!(benches, bench_parse);
criterion_main!(benches);
