//! The loaded document: the cell array, the program that derives cells, and
//! the columnar component layer that names cells for references and the
//! renderer.

use std::cell::OnceCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::components::ComponentKind;
use crate::dast::{Dast, NodeId, StrId, StringTable};
use crate::program::Program;

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

/// What a build knew about the document's shape, carried into the next
/// build so iteration counts and essential values survive.
#[derive(Debug, Clone, Default)]
pub struct Structure {
    /// (parent scope, repeat element, 1-based position) per scope; entry 0
    /// is the document. Ids are stable across rebuilds: the table only grows.
    pub scopes: Vec<(ScopeId, NodeId, u32)>,
    pub scope_index: HashMap<(ScopeId, NodeId, u32), ScopeId>,
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
}

/// Upper bound on build passes before the structure must have settled.
const MAX_PASSES: usize = 8;

pub const NONE: u32 = u32::MAX;
/// Set on a `child_list` entry whose low bits are a string id, not a component.
pub const TEXT_BIT: u32 = 1 << 31;

/// Components as parallel arrays indexed by `CompIdx`. Props are implicit:
/// component `c` of kind `k` owns `prop_cells[prop_base[c] + i]` for each
/// `i` in `k.prop_defs()`.
#[derive(Debug, Clone, Default)]
pub struct Components {
    pub kind: Vec<ComponentKind>,
    /// String id of the name, or `NONE`.
    pub name: Vec<StrId>,
    pub parent: Vec<CompIdx>,
    pub prop_base: Vec<u32>,
    pub prop_cells: Vec<CellIdx>,
    pub child_start: Vec<u32>,
    pub child_count: Vec<u32>,
    /// Component indices, or `TEXT_BIT | string id` for text children.
    pub child_list: Vec<u32>,
    /// DAST element each component came from (NONE if synthesized) and the
    /// scope it was created in. Together they identify a component across
    /// rebuilds, which lets a renderer keep its tree keyed by identity.
    pub node: Vec<u32>,
    pub scope: Vec<ScopeId>,
}

impl Components {
    pub fn len(&self) -> usize {
        self.kind.len()
    }

    pub fn is_empty(&self) -> bool {
        self.kind.is_empty()
    }

    pub fn heap_bytes(&self) -> usize {
        self.kind.capacity() * std::mem::size_of::<ComponentKind>()
            + 4 * (self.name.capacity()
                + self.parent.capacity()
                + self.prop_base.capacity()
                + self.prop_cells.capacity()
                + self.child_start.capacity()
                + self.child_count.capacity()
                + self.child_list.capacity()
                + self.node.capacity()
                + self.scope.capacity())
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Child<'a> {
    Component(CompIdx),
    Text(&'a str),
}

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
    pub comps: Components,
    /// Names and text, shared with the DAST they came from.
    pub strings: StringTable,
    /// Index of the root `<document>` component.
    pub root: CompIdx,
    pub structure: Structure,
    /// The document as loaded, kept for rebuilds.
    pub dast: Arc<Dast>,
    name_map: OnceCell<HashMap<String, CompIdx>>,
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

#[derive(Debug, Clone, Copy, Default)]
pub struct LoadTimings {
    pub deserialize: Duration,
    pub build: Duration,
    pub schedule: Duration,
    pub initial_compute: Duration,
    /// Build passes until repeat counts settled (1 without repeats).
    pub passes: u32,
    /// `Structure::structural_depth` of the settled document.
    pub structural_depth: u32,
}

impl Document {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(cells: Vec<f64>, n_essential: usize, n_fixed: usize, program: Program, comps: Components, strings: StringTable, root: CompIdx, structure: Structure, dast: Arc<Dast>) -> Self {
        Document { cells, n_essential, n_fixed, program, comps, strings, root, structure, dast, name_map: OnceCell::new() }
    }

    /// Load from DAST JSON and compute initial values.
    pub fn from_dast_json(json: &str) -> crate::Result<Document> {
        Ok(Self::load_timed(json.as_bytes())?.0)
    }

    /// Load from either wire format (JSON or binary), detected by content.
    pub fn from_bytes(bytes: &[u8]) -> crate::Result<Document> {
        Ok(Self::load_timed(bytes)?.0)
    }

    /// Load from either wire format, timing each stage separately. Build,
    /// schedule and compute repeat until every repeat's iteration count
    /// matches its `count` cell; the timings sum over passes.
    pub fn load_timed(bytes: &[u8]) -> crate::Result<(Document, LoadTimings)> {
        let mut t = LoadTimings::default();
        let clock = web_time::Instant::now();
        let dast = Arc::new(crate::dast::load(bytes)?);
        t.deserialize = clock.elapsed();
        let doc = Self::build_settled(dast, &mut t)?;
        Ok((doc, t))
    }

    pub fn from_dast(dast: Arc<Dast>) -> crate::Result<Document> {
        Self::build_settled(dast, &mut LoadTimings::default())
    }

    fn build_settled(dast: Arc<Dast>, t: &mut LoadTimings) -> crate::Result<Document> {
        let mut prior = crate::build::Prior::default();
        for _ in 0..MAX_PASSES {
            let clock = web_time::Instant::now();
            let unscheduled = crate::build::build(&dast, &prior)?;
            t.build += clock.elapsed();

            let clock = web_time::Instant::now();
            let mut doc = unscheduled.schedule(dast.clone())?;
            t.schedule += clock.elapsed();

            let clock = web_time::Instant::now();
            doc.recompute();
            t.initial_compute += clock.elapsed();
            t.passes += 1;
            if doc.structure_settled() {
                t.structural_depth = doc.structure.structural_depth;
                return Ok(doc);
            }
            prior = crate::build::Prior::take_from(&mut doc);
        }
        Err(crate::Error::UnstableStructure(MAX_PASSES))
    }

    /// Authoring warnings about the loaded document. Today: repeats whose
    /// count reads a cell inside another repeat's iterations, since each
    /// such link costs a full extra build pass and, unlike nesting, is
    /// avoidable (see `Structure::repeat_depths`).
    pub fn warnings(&self) -> Vec<String> {
        let st = &self.structure;
        let mut out = Vec::new();
        for ((r, &d), &cross) in st.repeats.iter().zip(&st.repeat_depths).zip(&st.repeat_cross_reads) {
            if cross {
                let name = self.name(r.comp).map(str::to_string).unwrap_or_else(|| format!("<repeatForSequence>#{}", r.comp));
                out.push(format!(
                    "repeat '{name}' has structural depth {d}: its count reads a cell inside another repeat's iterations, so a change there costs {d} build passes instead of one"
                ));
            }
        }
        out
    }

    /// Whether every repeat was expanded with the iteration count its
    /// `count` cell now holds.
    pub fn structure_settled(&self) -> bool {
        self.structure.repeats.iter().all(|r| self.repeat_count(r) == r.n)
    }

    /// The iteration count a repeat's `count` cell currently asks for.
    pub fn repeat_count(&self, r: &Repeat) -> u32 {
        let pi = ComponentKind::RepeatForSequence.prop_index("count").unwrap();
        let v = self.cells[self.comp_cells(r.comp)[pi] as usize];
        if v.is_nan() || v < 0.0 { 0 } else { v.min(u32::MAX as f64) as u32 }
    }

    /// Rebuild from the retained DAST, carrying iteration counts and
    /// essential values over. On error the document is left unchanged.
    pub fn rebuild(&mut self) -> crate::Result<()> {
        let profile = std::env::var_os("CELLS_BUILD_PROFILE").is_some();
        let dast = self.dast.clone();
        let clock = web_time::Instant::now();
        // The value store moves into the prior; on failure it moves back.
        let mut prior = crate::build::Prior::take_from(self);
        if profile {
            eprintln!("rebuild/prior: {:.2?}", clock.elapsed());
        }
        let result = (|| {
            for _ in 0..MAX_PASSES {
                let clock = web_time::Instant::now();
                let u = crate::build::build(&dast, &prior)?;
                if profile {
                    eprintln!("rebuild/build: {:.2?}", clock.elapsed());
                }
                let clock = web_time::Instant::now();
                let mut doc = u.schedule(dast.clone())?;
                if profile {
                    eprintln!("rebuild/schedule: {:.2?} (creation order valid: {})", clock.elapsed(), doc.program.in_creation_order);
                }
                let clock = web_time::Instant::now();
                doc.recompute();
                if profile {
                    eprintln!("rebuild/recompute: {:.2?}", clock.elapsed());
                }
                if doc.structure_settled() {
                    return Ok(doc);
                }
                prior = crate::build::Prior::take_from(&mut doc);
            }
            Err(crate::Error::UnstableStructure(MAX_PASSES))
        })();
        match result {
            Ok(doc) => {
                *self = doc;
                Ok(())
            }
            Err(e) => {
                prior.restore(self);
                Err(e)
            }
        }
    }

    pub fn is_essential(&self, cell: CellIdx) -> bool {
        (cell as usize) < self.n_essential
    }

    pub fn essential_cells(&self) -> &[f64] {
        &self.cells[..self.n_essential]
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
        self.request_with(&mut crate::eval::FullRecompute, requests)
    }

    /// `request` with an explicit recompute strategy.
    pub fn request_with(&mut self, evaluator: &mut (impl crate::eval::Evaluator + ?Sized), requests: &[Request]) -> Tick {
        let mut tick = Tick::default();
        for &r in requests {
            // An infinite ask is never meaningful state (NaN is: an emptied
            // input), and the current core rejects it; drop it before inverting.
            if r.value.is_infinite() {
                tick.dropped.push(r);
                continue;
            }
            match self.program.invert_to_essential(&self.cells, r.cell, r.value) {
                // Landed on a fixed cell (an iteration index, a collect count).
                Some((cell, _)) if !self.is_essential(cell) => tick.dropped.push(r),
                Some((cell, value)) => {
                    let old = self.cells[cell as usize];
                    if value != old && !(value.is_nan() && old.is_nan()) {
                        self.cells[cell as usize] = value;
                        if !tick.changed.contains(&cell) {
                            tick.changed.push(cell);
                        }
                    }
                }
                None => tick.dropped.push(r),
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

    /// Cells of a prop of a component inside an iteration, by scoped path:
    /// `scoped_cell(&["r", "3", "p"], "x")` names `$r[3].p.x`. For tests.
    pub fn scoped_component(&self, path: &[&str]) -> Option<CompIdx> {
        let mut comp = self.component(path[0])?;
        let mut i = 1;
        while i < path.len() {
            let r = self.structure.repeats.iter().find(|r| r.comp == comp)?;
            let k: usize = path[i].parse().ok()?;
            let scope = *r.iter_scopes.get(k.checked_sub(1)?)?;
            let name = path.get(i + 1).copied();
            let mut in_scope = self.children(comp).filter_map(|c| match c {
                Child::Component(c) if self.comp_scope_of(c) == Some(scope) && name.is_none_or(|n| self.name(c) == Some(n)) => Some(c),
                _ => None,
            });
            comp = in_scope.next()?;
            i += 2;
        }
        Some(comp)
    }

    fn comp_scope_of(&self, c: CompIdx) -> Option<ScopeId> {
        Some(self.comps.scope[c as usize])
    }

    // ---- component accessors --------------------------------------------

    pub fn n_components(&self) -> usize {
        self.comps.len()
    }

    pub fn kind(&self, c: CompIdx) -> ComponentKind {
        self.comps.kind[c as usize]
    }

    pub fn name(&self, c: CompIdx) -> Option<&str> {
        let s = self.comps.name[c as usize];
        (s != NONE).then(|| self.strings.get(s).trim())
    }

    pub fn parent(&self, c: CompIdx) -> Option<CompIdx> {
        let p = self.comps.parent[c as usize];
        (p != NONE).then_some(p)
    }

    pub fn children(&self, c: CompIdx) -> impl Iterator<Item = Child<'_>> + '_ {
        let (s, n) = (self.comps.child_start[c as usize] as usize, self.comps.child_count[c as usize] as usize);
        self.comps.child_list[s..s + n].iter().map(move |&e| {
            if e & TEXT_BIT != 0 { Child::Text(self.strings.get(e & !TEXT_BIT)) } else { Child::Component(e) }
        })
    }

    /// Cells of the single-cell props of `c`, in `kind.prop_defs()` order.
    pub fn comp_cells(&self, c: CompIdx) -> &[CellIdx] {
        let base = self.comps.prop_base[c as usize] as usize;
        &self.comps.prop_cells[base..base + self.kind(c).prop_defs().len()]
    }

    /// The first component with this plain name, in document order. Names
    /// inside a repeat template recur once per iteration; see
    /// `scoped_component` to pick an iteration.
    pub fn component(&self, name: &str) -> Option<CompIdx> {
        self.name_map
            .get_or_init(|| {
                let mut m = HashMap::new();
                for c in 0..self.comps.len() as CompIdx {
                    if let Some(n) = self.name(c) {
                        m.entry(n.to_string()).or_insert(c);
                    }
                }
                m
            })
            .get(name)
            .copied()
    }

    pub fn component_names(&self) -> impl Iterator<Item = (&str, CompIdx)> {
        (0..self.comps.len() as CompIdx).filter_map(|c| self.name(c).map(|n| (n, c)))
    }

    /// Cells of a prop, including virtual props such as a point's `coords`.
    pub fn prop_cells(&self, comp: CompIdx, prop: &str) -> Option<Vec<CellIdx>> {
        let kind = self.kind(comp);
        if let Some(parts) = kind.virtual_prop(prop) {
            return parts.iter().map(|p| self.prop_cells(comp, p).map(|v| v[0])).collect();
        }
        let i = kind.prop_index(prop)?;
        Some(vec![self.comp_cells(comp)[i]])
    }

    /// The single cell behind `name.prop`.
    pub fn cell(&self, name: &str, prop: &str) -> Option<CellIdx> {
        let cells = self.prop_cells(self.component(name)?, prop)?;
        (cells.len() == 1).then(|| cells[0])
    }

    pub fn value(&self, name: &str, prop: &str) -> Option<f64> {
        self.cell(name, prop).map(|c| self.cells[c as usize])
    }

    pub fn memory_estimate(&self) -> MemoryEstimate {
        MemoryEstimate {
            cells: self.cells.capacity() * 8,
            program: self.program.instrs.capacity() * std::mem::size_of::<crate::ops::Instr>() + self.program.producer.capacity() * 4 + self.program.arena.heap_bytes(),
            components: self.comps.heap_bytes(),
            strings: self.strings.heap_bytes(),
            structure: self.structure.scopes.capacity() * 12
                + self.structure.essential_slots.capacity() * 8
                + self.structure.values.iter().map(|r| r.capacity() * 16 + 24).sum::<usize>()
                + self.structure.scope_index.capacity() * 16,
            dast: self.dast.heap_bytes(),
        }
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
