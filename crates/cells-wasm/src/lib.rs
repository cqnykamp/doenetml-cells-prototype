//! Browser binding. The renderer reads cells through a `Float64Array` view
//! over wasm memory (see `cells_ptr`/`cells_len`), receives a one-time render
//! manifest, and writes with cell-addressed requests. See ADR 0001.

use cells_core::{DirtyClosure, DirtyScan, Document, Evaluator, FullRecompute, PointRequest, Request};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct Core {
    doc: Document,
    evaluator: Box<dyn Evaluator>,
    timings: cells_core::LoadTimings,
    last_dropped: u32,
    last_rebuilt: bool,
    last_rebuild_error: Option<String>,
    /// ms spent inside the rebuild during the last tick, if one happened.
    last_rebuild_ms: f64,
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
        Ok(Core { doc, evaluator, timings, last_dropped: 0, last_rebuilt: false, last_rebuild_error: None, last_rebuild_ms: 0.0 })
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
            r#"{{"deserialize":{},"build":{},"schedule":{},"initial_compute":{},"passes":{},"structural_depth":{}}}"#,
            t.deserialize.as_secs_f64() * 1e3,
            t.build.as_secs_f64() * 1e3,
            t.schedule.as_secs_f64() * 1e3,
            t.initial_compute.as_secs_f64() * 1e3,
            t.passes,
            t.structural_depth
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
    /// Stable identity across rebuilds: DAST node (NONE if synthesized) and scope.
    pub fn comp_node_ptr(&self) -> *const u32 {
        self.doc.comps.node.as_ptr()
    }
    pub fn comp_scope_ptr(&self) -> *const u32 {
        self.doc.comps.scope.as_ptr()
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

    /// Apply cell-addressed requests. Returns the changed cell indices. When
    /// the tick rebuilt the document (`last_rebuilt`), the list is empty and
    /// every pointer and length above must be re-read.
    pub fn request(&mut self, cells: &[u32], values: &[f64]) -> Vec<u32> {
        let reqs: Vec<Request> = cells.iter().zip(values).map(|(&cell, &value)| Request { cell, value }).collect();
        self.apply(&reqs, &[])
    }

    /// Points dragged together, as `x0, y0, x1, y1, ...`: one point group
    /// (ADR 0006), so a constrained point carries the others with it.
    pub fn request_points(&mut self, cells: &[u32], values: &[f64]) -> Vec<u32> {
        let pts: Vec<PointRequest> = cells.chunks_exact(2).zip(values.chunks_exact(2)).map(|(c, v)| PointRequest { cells: [c[0], c[1]], values: [v[0], v[1]] }).collect();
        self.apply(&[], &[pts])
    }

    fn apply(&mut self, reqs: &[Request], groups: &[Vec<PointRequest>]) -> Vec<u32> {
        let n_before = self.doc.program.len();
        let clock = web_time::Instant::now();
        let tick = self.doc.request_with_groups(self.evaluator.as_mut(), reqs, groups);
        self.last_dropped = tick.dropped.len() as u32;
        self.last_rebuilt = tick.rebuilt;
        self.last_rebuild_error = tick.rebuild_error;
        self.last_rebuild_ms = 0.0;
        if tick.rebuilt {
            // The whole tick is the rebuild when the program changed shape;
            // the evaluator's dependency tables are for the old program.
            self.last_rebuild_ms = clock.elapsed().as_secs_f64() * 1e3;
            let _ = n_before;
            let name = self.evaluator.name().to_string();
            self.set_evaluator(&name).unwrap();
        }
        tick.changed
    }

    // ---- test adapter -----------------------------------------------------
    //
    // The current core's vitest suites run against this core through an
    // adapter (plan 3). These calls answer by name so the adapter needs no
    // knowledge of the column layout.

    /// Component index of a dotted path as the tests write it
    /// (`"g.Ps[2]"`), or `u32::MAX`.
    pub fn resolve_path(&self, path: &str) -> u32 {
        self.doc.resolve_path(path).unwrap_or(u32::MAX)
    }

    pub fn component_tag(&self, idx: u32) -> Option<String> {
        if (idx as usize) < self.doc.n_components() { Some(self.doc.kind(idx).tag().to_string()) } else { None }
    }

    /// Cells of a prop by name, including virtual (`coords`, `center`) and
    /// array (`points`, `vertices`) props; empty if no such prop.
    pub fn prop_cells(&self, idx: u32, prop: &str) -> Vec<u32> {
        if (idx as usize) >= self.doc.n_components() {
            return Vec::new();
        }
        self.doc.prop_cells(idx, prop).unwrap_or_default()
    }

    pub fn cell_value(&self, cell: u32) -> f64 {
        self.doc.cells.get(cell as usize).copied().unwrap_or(f64::NAN)
    }

    /// Whether a cell is essential (state), fixed (a constant), or derived.
    pub fn cell_class(&self, cell: u32) -> String {
        if self.doc.is_essential(cell) {
            "essential".into()
        } else if self.doc.is_fixed(cell) {
            "fixed".into()
        } else {
            "derived".into()
        }
    }

    /// Components named at the top level, as JSON `[[name, idx], ...]`, for
    /// adapter diagnostics.
    pub fn component_names_json(&self) -> String {
        serde_json::to_string(&self.doc.component_names().collect::<Vec<_>>()).unwrap()
    }

    pub fn last_dropped(&self) -> u32 {
        self.last_dropped
    }

    pub fn last_rebuilt(&self) -> bool {
        self.last_rebuilt
    }

    pub fn last_rebuild_ms(&self) -> f64 {
        self.last_rebuild_ms
    }

    pub fn last_rebuild_error(&self) -> Option<String> {
        self.last_rebuild_error.clone()
    }

    /// Text of the expression a math cell holds, for display.
    pub fn expr_text(&self, cell: u32) -> String {
        self.doc.math_text(cell)
    }

    /// LaTeX of the expression a math cell holds.
    pub fn expr_latex(&self, cell: u32) -> String {
        self.doc.math_latex(cell)
    }

    /// Build passes the load took to settle repeat counts.
    pub fn passes(&self) -> u32 {
        self.timings.passes
    }

    /// Authoring warnings for the loaded document, JSON array of strings.
    pub fn warnings_json(&self) -> String {
        serde_json::to_string(&self.doc.warnings()).unwrap()
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
