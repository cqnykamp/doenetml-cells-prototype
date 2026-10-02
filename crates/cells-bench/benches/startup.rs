use cells_core::Document;
use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion, Throughput};

fn startup(c: &mut Criterion) {
    for (spec, json) in cells_bench::fixtures() {
        let dast = cells_core::dast::parse_json(&json).unwrap();
        let doc = Document::from_dast(&dast).unwrap();
        let n_cells = doc.cells.len() as u64;

        let mut g = c.benchmark_group("startup");
        g.sample_size(20).throughput(Throughput::Elements(n_cells));
        g.bench_with_input(BenchmarkId::new("deserialize", &spec), &json, |b, json| {
            b.iter(|| cells_core::dast::parse_json(json).unwrap())
        });
        g.bench_with_input(BenchmarkId::new("build", &spec), &dast, |b, dast| b.iter(|| cells_core::build::build(dast).unwrap()));
        g.bench_with_input(BenchmarkId::new("schedule", &spec), &dast, |b, dast| {
            b.iter_batched(|| cells_core::build::build(dast).unwrap(), |u| u.schedule().unwrap(), BatchSize::SmallInput)
        });
        g.bench_with_input(BenchmarkId::new("initial_compute", &spec), &doc, |b, doc| {
            b.iter_batched(|| doc.clone(), |mut d| { d.recompute(); d }, BatchSize::SmallInput)
        });
        g.bench_with_input(BenchmarkId::new("total_from_json", &spec), &json, |b, json| {
            b.iter(|| Document::from_dast_json(json).unwrap())
        });
        g.finish();
    }
}

criterion_group!(benches, startup);
criterion_main!(benches);
