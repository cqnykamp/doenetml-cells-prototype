//! Browser binding. The renderer reads cells through a `Float64Array` view
//! over wasm memory (see `cells_ptr`/`cells_len`), receives a one-time render
//! manifest, and writes with cell-addressed requests. See ADR 0001.

use cells_core::{Child, DirtyClosure, DirtyScan, Document, Evaluator, FullRecompute, Request};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct Core {
    doc: Document,
    evaluator: Box<dyn Evaluator>,
    timings: cells_core::LoadTimings,
    last_dropped: u32,
}

#[wasm_bindgen]
impl Core {
    /// Load a document from DAST JSON and compute its initial values.
    #[wasm_bindgen(constructor)]
    pub fn new(dast_json: &str) -> Result<Core, JsError> {
        console_error_panic_hook::set_once();
        let (doc, timings) = Document::load_timed(dast_json).map_err(|e| JsError::new(&e.to_string()))?;
        let evaluator: Box<dyn Evaluator> = Box::new(DirtyClosure::new(&doc.program, doc.cells.len()));
        Ok(Core { doc, evaluator, timings, last_dropped: 0 })
    }

    /// Pointer to the cell array inside wasm memory. Valid until the next
    /// call that may allocate; callers re-derive their view after each call.
    pub fn cells_ptr(&self) -> *const f64 {
        self.doc.cells.as_ptr()
    }

    pub fn cells_len(&self) -> usize {
        self.doc.cells.len()
    }

    pub fn n_essential(&self) -> usize {
        self.doc.n_essential
    }

    pub fn n_instrs(&self) -> usize {
        self.doc.program.len()
    }

    /// Stage timings from loading, in milliseconds, as JSON.
    pub fn load_timings_json(&self) -> String {
        let t = &self.timings;
        format!(
            r#"{{"deserialize":{},"build":{},"schedule":{},"initial_compute":{}}}"#,
            t.deserialize.as_secs_f64() * 1e3,
            t.build.as_secs_f64() * 1e3,
            t.schedule.as_secs_f64() * 1e3,
            t.initial_compute.as_secs_f64() * 1e3
        )
    }

    /// The render manifest: every component with its kind, name, prop cell
    /// indices and children, as JSON. Sent once at load. Written directly
    /// to a string: building a serde_json::Value tree first cost more than
    /// the whole core load on 100k-component documents.
    pub fn manifest_json(&self) -> String {
        use std::fmt::Write;
        let doc = &self.doc;
        let mut out = String::with_capacity(doc.components.len() * 96);
        let _ = write!(
            out,
            r#"{{"root":{},"nEssential":{},"nCells":{},"components":["#,
            doc.root,
            doc.n_essential,
            doc.cells.len()
        );
        for (i, c) in doc.components.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            let _ = write!(out, r#"{{"kind":"{}","name":"#, c.kind.tag());
            match &c.name {
                Some(n) => out.push_str(&serde_json::to_string(n).unwrap()),
                None => out.push_str("null"),
            }
            out.push_str(r#","parent":"#);
            match c.parent {
                Some(p) => {
                    let _ = write!(out, "{p}");
                }
                None => out.push_str("null"),
            }
            out.push_str(r#","props":{"#);
            for (j, p) in c.props.iter().enumerate() {
                if j > 0 {
                    out.push(',');
                }
                let _ = write!(out, r#""{}":{}"#, p.name, p.cells[0]);
            }
            out.push_str(r#"},"children":["#);
            for (j, ch) in c.children.iter().enumerate() {
                if j > 0 {
                    out.push(',');
                }
                match ch {
                    Child::Component(idx) => {
                        let _ = write!(out, r#"{{"c":{idx}}}"#);
                    }
                    Child::Text(t) => {
                        let _ = write!(out, r#"{{"t":{}}}"#, serde_json::to_string(t).unwrap());
                    }
                }
            }
            out.push_str("]}");
        }
        out.push_str("]}");
        out
    }

    /// Apply cell-addressed requests. Returns the changed cell indices.
    pub fn request(&mut self, cells: &[u32], values: &[f64]) -> Vec<u32> {
        let reqs: Vec<Request> = cells.iter().zip(values).map(|(&cell, &value)| Request { cell, value }).collect();
        let tick = self.doc.request_with(self.evaluator.as_mut(), &reqs);
        self.last_dropped = tick.dropped.len() as u32;
        tick.changed
    }

    pub fn last_dropped(&self) -> u32 {
        self.last_dropped
    }

    /// Choose the recompute strategy: "full", "dirty-scan" or "dirty-closure".
    pub fn set_evaluator(&mut self, name: &str) -> Result<(), JsError> {
        self.evaluator = match name {
            "full" => Box::new(FullRecompute),
            "dirty-scan" => Box::new(DirtyScan::new(self.doc.cells.len())),
            "dirty-closure" => Box::new(DirtyClosure::new(&self.doc.program, self.doc.cells.len())),
            other => return Err(JsError::new(&format!("unknown evaluator {other}"))),
        };
        Ok(())
    }

    pub fn evaluator_name(&self) -> String {
        self.evaluator.name().to_string()
    }
}
