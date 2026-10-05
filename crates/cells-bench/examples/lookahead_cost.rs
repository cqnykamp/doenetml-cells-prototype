//! Time one lookahead against one whole tick on a fixture: the circle's
//! center drag asks `realize` about its three points before writing them.
use cells_core::{Document, Request};
use std::time::Instant;

fn main() {
    let spec = std::env::args().nth(1).unwrap_or_else(|| "circles3-10000".into());
    let json = std::fs::read_to_string(cells_bench::fixtures_dir().join(format!("{spec}.json"))).unwrap();
    let mut doc = Document::from_dast_json(&json).unwrap();
    let cx = doc.cell("k0", "cx").unwrap();
    let pts: Vec<(u32, f64)> = ["a0", "b0", "c0"].iter().flat_map(|n| [(doc.cell(n, "x").unwrap(), 1.0), (doc.cell(n, "y").unwrap(), 1.0)]).collect();
    let reps = 200;
    let mut out = Vec::new();
    let t = Instant::now();
    for _ in 0..reps {
        out.clear();
        doc.program.realize(&doc.cells, doc.n_essential, &pts, &mut out);
    }
    let look = t.elapsed() / reps;
    let t = Instant::now();
    for i in 0..reps {
        doc.request(&[Request { cell: cx, value: 3.0 + (i % 2) as f64 }]);
    }
    let tick = t.elapsed() / reps;
    println!("{spec}: cells {} instrs {}  lookahead {:?}  full tick {:?}", doc.cells.len(), doc.program.len(), look, tick);
}
