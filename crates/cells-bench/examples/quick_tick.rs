//! Mean full-recompute tick of a fixture's drag, split into its stages, for
//! quick regression checks.
use cells_core::{Document, Request};
use std::time::Instant;

fn main() {
    for spec in std::env::args().skip(1) {
        let json = std::fs::read_to_string(cells_bench::fixtures_dir().join(format!("{spec}.json"))).unwrap();
        let mut doc = Document::from_dast_json(&json).unwrap();
        let target = cells_bench::drag_target(&doc, &spec);
        let reps = 100;
        let t = Instant::now();
        for i in 0..reps {
            doc.request(&[Request { cell: target, value: 3.0 + (i % 2) as f64 }]);
        }
        let tick = t.elapsed() / reps;
        let t = Instant::now();
        for i in 0..reps {
            std::hint::black_box(doc.program.invert_requests(&doc.cells, doc.n_essential, &[Request { cell: target, value: 3.0 + (i % 2) as f64 }], &[]));
        }
        let invert = t.elapsed() / reps;
        let mut changed = Vec::new();
        let t = Instant::now();
        for _ in 0..reps {
            changed.clear();
            doc.program.run_all_tracking(&mut doc.cells, &mut changed);
        }
        let tracking = t.elapsed() / reps;
        println!("{spec}: tick {tick:.2?}, invert {invert:.2?}, run_all_tracking {tracking:.2?}");
    }
}
