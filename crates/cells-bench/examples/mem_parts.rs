//! Heap estimate by part and load time by stage, per fixture.
use cells_core::Document;
fn main() {
    for spec in std::env::args().skip(1) {
        let bytes = cells_bench::fixture_binary(&spec).unwrap();
        let (doc, t) = Document::load_timed(&bytes).unwrap();
        let m = doc.memory_estimate();
        println!("{spec:18} cells {:6.2} program {:6.2} comps {:6.2} strings {:6.2} structure {:6.2} dast {:6.2} MB | deser {:6.1} build {:6.1} sched {:6.1} compute {:6.1} ms", m.cells as f64/1e6, m.program as f64/1e6, m.components as f64/1e6, m.strings as f64/1e6, m.structure as f64/1e6, m.dast as f64/1e6, t.deserialize.as_secs_f64()*1e3, t.build.as_secs_f64()*1e3, t.schedule.as_secs_f64()*1e3, t.initial_compute.as_secs_f64()*1e3);
    }
}
