//! Browser binding. The renderer reads cells through a `Float64Array` view
//! over wasm memory (see `cells_ptr`/`cells_len`), receives a one-time render
//! manifest, and writes with cell-addressed requests. See ADR 0001.

use cells_core::{DirtyClosure, DirtyScan, Document, Evaluator, FullRecompute, Request};
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
    /// Load a document from either wire format (DAST JSON or binary CDST,
    /// detected by content) and compute its initial values.
    #[wasm_bindgen(constructor)]
    pub fn new(dast: &[u8]) -> Result<Core, JsError> {
        console_error_panic_hook::set_once();
        let (doc, timings) = Document::load_timed(dast).map_err(|e| JsError::new(&e.to_string()))?;
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

    // ---- component tables -------------------------------------------------
    //
    // The renderer reads the columnar component layer directly through typed
    // array views over wasm memory, the same way it reads cells. There is no
    // serialized manifest. Pointers are valid until the next allocating call.

    pub fn root(&self) -> u32 {
        self.doc.root
    }

    pub fn n_components(&self) -> usize {
        self.doc.n_components()
    }

    /// `u8` per component: the `ComponentKind` discriminant, in the order of
    /// `kind_tags()`.
    pub fn comp_kind_ptr(&self) -> *const u8 {
        self.doc.comps.kind.as_ptr() as *const u8
    }

    /// Tag names indexed by kind discriminant, JSON array.
    pub fn kind_tags(&self) -> String {
        serde_json::to_string(&cells_core::components::ComponentKind::ALL.iter().map(|k| k.tag()).collect::<Vec<_>>()).unwrap()
    }

    /// Prop names per kind, JSON array of arrays, in the order of `kind_tags()`.
    pub fn kind_props(&self) -> String {
        serde_json::to_string(&cells_core::components::ComponentKind::ALL.iter().map(|k| k.prop_defs().iter().map(|p| p.name).collect::<Vec<_>>()).collect::<Vec<_>>()).unwrap()
    }

    pub fn comp_name_ptr(&self) -> *const u32 {
        self.doc.comps.name.as_ptr()
    }
    pub fn comp_parent_ptr(&self) -> *const u32 {
        self.doc.comps.parent.as_ptr()
    }
    pub fn comp_prop_base_ptr(&self) -> *const u32 {
        self.doc.comps.prop_base.as_ptr()
    }
    pub fn prop_cells_ptr(&self) -> *const u32 {
        self.doc.comps.prop_cells.as_ptr()
    }
    pub fn prop_cells_len(&self) -> usize {
        self.doc.comps.prop_cells.len()
    }
    pub fn comp_child_start_ptr(&self) -> *const u32 {
        self.doc.comps.child_start.as_ptr()
    }
    pub fn comp_child_count_ptr(&self) -> *const u32 {
        self.doc.comps.child_count.as_ptr()
    }
    pub fn child_list_ptr(&self) -> *const u32 {
        self.doc.comps.child_list.as_ptr()
    }
    pub fn child_list_len(&self) -> usize {
        self.doc.comps.child_list.len()
    }
    /// String table: `n_strings + 1` offsets into the UTF-8 byte blob.
    pub fn string_offsets_ptr(&self) -> *const u32 {
        self.doc.strings.offsets.as_ptr()
    }
    pub fn n_strings(&self) -> usize {
        self.doc.strings.len()
    }
    pub fn string_bytes_ptr(&self) -> *const u8 {
        self.doc.strings.bytes.as_ptr()
    }
    pub fn string_bytes_len(&self) -> usize {
        self.doc.strings.bytes.len()
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
