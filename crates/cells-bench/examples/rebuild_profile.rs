//! Where a rebuild spends its time, per phase, for one fixture.
use cells_core::{Document, Request};
use std::time::Instant;

fn main() {
    let spec = std::env::args().nth(1).unwrap_or_else(|| "repeat-10000".into());
    let json = std::fs::read_to_string(cells_bench::fixtures_dir().join(format!("{spec}.json"))).unwrap();
    let (doc, t) = Document::load_timed(json.as_bytes()).unwrap();
    println!("{spec}: load passes={} deserialize={:.2?} build={:.2?} schedule={:.2?} compute={:.2?}", t.passes, t.deserialize, t.build, t.schedule, t.initial_compute);
    println!("  cells={} essential={} fixed={} instrs={} components={}", doc.cells.len(), doc.n_essential, doc.n_fixed, doc.program.len(), doc.n_components());
    let mut doc = doc;
    let n = doc.value("n", "value").unwrap();
    for round in 0..3 {
        let target = doc.cell("n", "value").unwrap();
        let value = if round % 2 == 0 { n - 1.0 } else { n };
        // Reproduce Document::rebuild step by step to time it.
        let c = Instant::now();
        let mut d2 = doc.clone();
        d2.cells[target as usize] = value;
        d2.recompute();
        let t_tick = c.elapsed();
        let c = Instant::now();
        let prior = cells_core::build::Prior::from_document(&d2);
        let t_prior = c.elapsed();
        let c = Instant::now();
        let mut engine = d2.program.sym.engine.borrow().box_clone();
        let u = cells_core::build::build(&d2.dast, &prior, &mut *engine).unwrap();
        let t_build = c.elapsed();
        let c = Instant::now();
        let mut d3 = u.schedule(d2.dast.clone(), &mut engine).unwrap();
        let t_sched = c.elapsed();
        let c = Instant::now();
        d3.recompute();
        let t_comp = c.elapsed();
        println!("  rebuild to {value}: recompute={t_tick:.2?} prior={t_prior:.2?} build={t_build:.2?} schedule={t_sched:.2?} compute={t_comp:.2?} settled={}", d3.structure_settled());
        let c = Instant::now();
        let tick = doc.request(&[Request { cell: target, value }]);
        println!("  whole request: {:.2?} rebuilt={}", c.elapsed(), tick.rebuilt);
    }
}
