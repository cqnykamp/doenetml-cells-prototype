//! The loaded document: the cell array, the program that derives cells, and
//! the columnar component layer that names cells for references and the
//! renderer.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use cells_sym::SymEngine;

use crate::components::{ComponentKind, prop};
use crate::dast::{Dast, NodeId, StrId, StringTable};
use crate::invert::PointRequest;
use crate::program::Program;

mod load;
mod paths;
mod sections;
mod sticky;
mod table;

pub use load::{LoadOptions, LoadTimings};
pub use table::{Child, ComponentTable};

pub type CellIdx = u32;
pub type CompIdx = u32;
/// A name scope: 0 is the document, every other scope is one iteration of a
/// repeat (see `build.rs`).
pub type ScopeId = u32;

/// One expanded `<repeatForSequence>`.
#[derive(Debug, Clone)]
pub struct Repeat {
    pub comp: CompIdx,
    pub node: NodeId,
    /// Scope the repeat element sits in.
    pub scope: ScopeId,
    /// One scope per iteration, in order.
    pub iter_scopes: Vec<ScopeId>,
    /// Iterations the build used; a differing `count` cell triggers a rebuild.
    pub n: u32,
}

/// Every scope a build has made: (parent scope, repeat or choice element,
/// 1-based position) per scope, indexed by `ScopeId`; entry 0 is the
/// document. Ids are stable across rebuilds: the table only grows.
#[derive(Debug, Clone)]
pub struct ScopeTable {
    entries: Vec<(ScopeId, NodeId, u32)>,
    index: HashMap<(ScopeId, NodeId, u32), ScopeId>,
}

impl Default for ScopeTable {
    fn default() -> Self {
        ScopeTable { entries: vec![(NONE, NONE, 0)], index: HashMap::new() }
    }
}

impl ScopeTable {
    /// The id of position `k` of element `node` under `parent`, created on
    /// first use.
    pub fn get_or_insert(&mut self, parent: ScopeId, node: NodeId, k: u32) -> ScopeId {
        if let Some(&s) = self.index.get(&(parent, node, k)) {
            return s;
        }
        self.entries.push((parent, node, k));
        let s = (self.entries.len() - 1) as ScopeId;
        self.index.insert((parent, node, k), s);
        s
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn heap_bytes(&self) -> usize {
        self.entries.capacity() * 12 + self.index.capacity() * 16
    }
}

impl std::ops::Index<ScopeId> for ScopeTable {
    type Output = (ScopeId, NodeId, u32);
    fn index(&self, s: ScopeId) -> &Self::Output {
        &self.entries[s as usize]
    }
}

/// What a build knew about the document's shape, carried into the next
/// build so iteration counts and essential values survive.
#[derive(Debug, Clone, Default)]
pub struct Structure {
    pub scopes: ScopeTable,
    /// Per essential cell, in cell order: its essential key as (scope,
    /// template slot). The template slot identifies the element and prop
    /// within the scope's template. See `CONTEXT.md`, essential key.
    pub essential_slots: Vec<(ScopeId, u32)>,
    /// `values[scope][template slot]`: the last value of every essential
    /// cell that has ever existed, so an iteration that disappears and
    /// reappears returns as it was left. Rows fill lazily.
    pub values: Vec<Vec<Option<f64>>>,
    pub repeats: Vec<Repeat>,
    pub counts_used: Vec<u32>,
    /// Per repeat (same order as `repeats`): how many repeats must be
    /// expanded in sequence before this one's count is known, plus one.
    /// Nesting adds one; a count that reads a cell inside another repeat's
    /// iterations adds one. Load takes `structural_depth + 1` passes.
    pub repeat_depths: Vec<u32>,
    /// Per repeat: whether its count reads a cell inside another repeat's
    /// iterations (the avoidable kind of depth, reported by `warnings`).
    pub repeat_cross_reads: Vec<bool>,
    /// The largest `repeat_depths` entry; 0 without repeats.
    pub structural_depth: u32,
    /// The document seed load-time choices draw from (plan 6).
    pub seed: u64,
    /// Curves sample through the engine, not compiled tapes (see
    /// `LoadOptions`).
    pub sample_with_engine: bool,
}

/// Upper bound on build passes before the structure must have settled.
const MAX_PASSES: usize = 8;

pub const NONE: u32 = u32::MAX;
/// Set on a `child_list` entry whose low bits are a string id, not a component.
pub const TEXT_BIT: u32 = 1 << 31;

#[derive(Debug, Clone)]
pub struct Document {
    /// All cells. Essential cells come first, then fixed, then derived.
    pub cells: Vec<f64>,
    /// Number of essential cells; `cells[..n_essential]` are essential.
    pub n_essential: usize,
    /// Fixed cells follow the essential ones: constants that are not state
    /// (iteration indices, collect counts, the missing-referent NaN).
    pub n_fixed: usize,
    pub program: Program,
    pub comps: ComponentTable,
    /// Names and text, shared with the DAST they came from.
    pub strings: StringTable,
    /// Index of the root `<document>` component.
    pub root: CompIdx,
    pub structure: Structure,
    /// The document as loaded, kept for rebuilds.
    pub dast: Arc<Dast>,
    /// Sticky groups as cells, for the request pre-pass (plan 4).
    sticky: Vec<sticky::StickyTable>,
}

/// A renderer's ask to change one cell to a value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Request {
    pub cell: CellIdx,
    pub value: f64,
}

/// What one tick changed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tick {
    /// Cells whose value changed, essential ones first in request order,
    /// then derived ones in schedule order. No duplicates within each part.
    pub changed: Vec<CellIdx>,
    /// Requests that were dropped because an inverse was undefined or the
    /// request landed on a fixed cell.
    pub dropped: Vec<Request>,
    /// The tick changed a structural cell and the document was rebuilt:
    /// cell indices and the component table are new, `changed` is empty.
    pub rebuilt: bool,
    /// The rebuild failed and the document is unchanged from before it.
    pub rebuild_error: Option<String>,
}

impl Document {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(cells: Vec<f64>, n_essential: usize, n_fixed: usize, program: Program, comps: ComponentTable, strings: StringTable, root: CompIdx, structure: Structure, dast: Arc<Dast>) -> Self {
        let mut doc = Document {
            cells,
            n_essential,
            n_fixed,
            program,
            comps,
            strings,
            root,
            structure,
            dast,
            sticky: Vec::new(),
        };
        doc.sticky = doc.sticky_tables();
        doc
    }

    /// Name of the symbolic engine ("A" or "R").
    pub fn engine_name(&self) -> &'static str {
        self.program.sym.engine.borrow().name()
    }

    /// Parse math text (what a student typed) into the engine: the value to
    /// request on a mathInput's `expr` cell.
    pub fn parse_math(&self, text: &str) -> std::result::Result<f64, String> {
        Ok(self.program.sym.engine.borrow_mut().parse(text)? as f64)
    }

    /// The expression a math cell holds, as text; empty when blank.
    pub fn math_text(&self, cell: CellIdx) -> String {
        let h = self.cells[cell as usize];
        if h.is_nan() { String::new() } else { self.program.sym.engine.borrow().text(h as cells_sym::Handle) }
    }

    /// A `<mathInput>` holds any math value, infinity included; every other
    /// request site rejects an infinite ask as the current core does.
    fn accepts_infinity(&self, cell: CellIdx) -> bool {
        (0..self.comps.len() as CompIdx).any(|c| self.kind(c) == ComponentKind::MathInput && self.comp_cells(c)[prop::math_input::VALUE] == cell)
    }

    pub fn is_essential(&self, cell: CellIdx) -> bool {
        (cell as usize) < self.n_essential
    }

    /// A constant that is not state (an iteration index, a math handle, a
    /// `fixed` value): fixed cells follow the essential ones.
    pub fn is_fixed(&self, cell: CellIdx) -> bool {
        (self.n_essential..self.n_essential + self.n_fixed).contains(&(cell as usize))
    }

    /// Recompute all derived cells from the essential cells.
    pub fn recompute(&mut self) {
        self.program.run_all(&mut self.cells);
    }

    /// Write an essential cell and recompute. Panics if `cell` is derived.
    pub fn set_essential(&mut self, cell: CellIdx, value: f64) {
        assert!(self.is_essential(cell), "cell {cell} is derived");
        self.cells[cell as usize] = value;
        self.recompute();
    }

    /// Apply requests: invert each to an essential cell (later requests win
    /// when two land on one cell), recompute, and report what changed.
    pub fn request(&mut self, requests: &[Request]) -> Tick {
        self.request_with_groups(&mut crate::eval::FullRecompute, requests, &[])
    }

    /// Apply a point group: points dragged together, which keep their shape
    /// when one of them is constrained (ADR 0006).
    pub fn request_points(&mut self, points: &[PointRequest]) -> Tick {
        self.request_with_groups(&mut crate::eval::FullRecompute, &[], &[points.to_vec()])
    }

    /// Scalar requests and point groups in one tick.
    pub fn request_with_groups(&mut self, evaluator: &mut (impl crate::eval::Evaluator + ?Sized), requests: &[Request], groups: &[Vec<PointRequest>]) -> Tick {
        let mut tick = Tick::default();
        // An infinite ask is never meaningful state (NaN is: an emptied
        // input), and the current core rejects it; drop it before inverting.
        let (mut finite, infinite): (Vec<Request>, Vec<Request>) = requests.iter().partition(|r| !r.value.is_infinite() || self.accepts_infinity(r.cell));
        tick.dropped.extend(infinite);
        let mut finite_groups: Vec<Vec<PointRequest>> = Vec::with_capacity(groups.len());
        for g in groups {
            if g.iter().any(|p| p.values.iter().any(|v| v.is_infinite())) {
                tick.dropped.extend(g.iter().map(|p| Request { cell: p.cells[0], value: p.values[0] }));
            } else {
                finite_groups.push(g.clone());
            }
        }
        self.snap_sticky(&mut finite, &mut finite_groups);
        let inv = self.program.invert_requests(&self.cells, self.n_essential, &finite, &finite_groups);
        tick.dropped.extend(inv.dropped);
        for (cell, value) in inv.writes {
            let old = self.cells[cell as usize];
            if value != old && !(value.is_nan() && old.is_nan()) {
                self.cells[cell as usize] = value;
                tick.changed.push(cell);
            }
        }
        if !tick.changed.is_empty() {
            evaluator.recompute(&self.program, &mut self.cells, &mut tick.changed);
            if !self.structure_settled() {
                match self.rebuild() {
                    Ok(()) => {
                        tick.rebuilt = true;
                        tick.changed.clear();
                    }
                    Err(e) => tick.rebuild_error = Some(e.to_string()),
                }
            }
        }
        tick
    }
}

/// Heap footprint of a loaded document, by part.
#[derive(Debug, Clone, Copy, Default)]
pub struct MemoryEstimate {
    pub cells: usize,
    pub program: usize,
    pub components: usize,
    pub strings: usize,
    /// Scope table and essential keys kept for rebuilds.
    pub structure: usize,
    /// The retained DAST, kept for rebuilds (its string table is shared
    /// with `strings` and counted there only once by `total`).
    pub dast: usize,
}

impl MemoryEstimate {
    pub fn total(&self) -> usize {
        self.cells + self.program + self.components + self.strings + self.structure + self.dast
    }
}
