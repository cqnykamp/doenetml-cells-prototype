//! Plan 6 measurements: per fixture and mechanism, load time and passes,
//! heap estimate, the median tick that flips every choice, and the median
//! tick of a drag no choice reads. JSON lines.
//!
//! Usage: `choice_bench <reps> <spec>...`; each spec runs under
//! `CELLS_CHOICE=built` and `=rebuild` (once for documents without a
//! reactive choice).
use cells_core::{Document, Request};
use std::time::Instant;

fn median(mut xs: Vec<f64>) -> f64 {
    if xs.is_empty() {
        return f64::NAN;
    }
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    xs[xs.len() / 2]
}

/// The input that flips every choice, and the two values it alternates.
fn flip_input(spec: &str) -> Option<(&'static str, [f64; 2])> {
    match spec.split('-').next().unwrap() {
        "wording" | "wordingflat" => Some(("n", [1.0, -1.0])),
        "adventure" => Some(("path", [1.0, 2.0])),
        "choicesweep" | "choicecurves" => Some(("n", [1.0, 2.0])),
        _ => None,
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let reps: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(20);
    for spec in args {
        let bytes = cells_bench::fixture_binary(&spec).unwrap_or_else(|| std::fs::read(cells_bench::fixtures_dir().join(format!("{spec}.json"))).unwrap());
        let reactive = !spec.starts_with("select") && !spec.ends_with("flat") && !spec.contains("flat-");
        let mechanisms: &[&str] = if reactive { &["built", "rebuild"] } else { &["-"] };
        for &m in mechanisms {
            // SAFETY: single-threaded; the builder reads it on each load.
            unsafe { std::env::set_var("CELLS_CHOICE", m) };
            let mut loads = Vec::new();
            let mut passes = 0;
            for _ in 0..reps.clamp(3, 7) {
                let t = Instant::now();
                let (doc, timings) = Document::load_timed(&bytes).unwrap();
                loads.push(t.elapsed().as_secs_f64() * 1e3);
                passes = timings.passes;
                std::hint::black_box(doc);
            }
            let mut doc = Document::from_bytes(&bytes).unwrap();
            let m_est = doc.memory_estimate();
            let (cells, instrs, comps) = (doc.cells.len(), doc.program.len(), doc.n_components());
            let mut flips = Vec::new();
            let mut rebuilt = false;
            if let Some((input, values)) = flip_input(&spec) {
                for i in 0..reps {
                    let cell = doc.cell(input, "value").unwrap();
                    let t = Instant::now();
                    let tick = doc.request(&[Request { cell, value: values[(i + 1) % 2] }]);
                    flips.push(t.elapsed().as_secs_f64() * 1e3);
                    rebuilt |= tick.rebuilt;
                    assert!(tick.rebuild_error.is_none(), "{spec}: {:?}", tick.rebuild_error);
                }
            }
            let mut drags = Vec::new();
            if let Some(target) = doc.cell("p0", "x") {
                for i in 0..reps {
                    let t = Instant::now();
                    std::hint::black_box(doc.request(&[Request { cell: target, value: 3.0 + (i % 2) as f64 }]));
                    drags.push(t.elapsed().as_secs_f64() * 1e3);
                }
            }
            println!(
                "{}",
                serde_json::json!({
                    "spec": spec, "mechanism": m, "load_ms": median(loads), "passes": passes,
                    "bytes": m_est.total(), "cells": cells, "instrs": instrs, "components": comps,
                    "flip_ms": median(flips), "flip_rebuilt": rebuilt, "drag_ms": median(drags),
                })
            );
        }
    }
}
