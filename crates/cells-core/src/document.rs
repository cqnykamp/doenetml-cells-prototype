//! The loaded document: the cell array, the program that derives cells, and
//! the columnar component layer that names cells for references and the
//! renderer.

use std::cell::OnceCell;
use std::collections::HashMap;
use std::time::Duration;

use crate::components::ComponentKind;
use crate::dast::{StrId, StringTable};
use crate::program::Program;

pub type CellIdx = u32;
pub type CompIdx = u32;

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
                + self.child_list.capacity())
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Child<'a> {
    Component(CompIdx),
    Text(&'a str),
}

#[derive(Debug, Clone)]
pub struct Document {
    /// All cells. Essential cells come first, then derived cells.
    pub cells: Vec<f64>,
    /// Number of essential cells; `cells[..n_essential]` are essential.
    pub n_essential: usize,
    pub program: Program,
    pub comps: Components,
    /// Names and text, shared with the DAST they came from.
    pub strings: StringTable,
    /// Index of the root `<document>` component.
    pub root: CompIdx,
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
    /// Requests that were dropped because an inverse was undefined.
    pub dropped: Vec<Request>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct LoadTimings {
    pub deserialize: Duration,
    pub build: Duration,
    pub schedule: Duration,
    pub initial_compute: Duration,
}

impl Document {
    pub(crate) fn new(cells: Vec<f64>, n_essential: usize, program: Program, comps: Components, strings: StringTable, root: CompIdx) -> Self {
        Document { cells, n_essential, program, comps, strings, root, name_map: OnceCell::new() }
    }

    /// Load from DAST JSON and compute initial values.
    pub fn from_dast_json(json: &str) -> crate::Result<Document> {
        Ok(Self::load_timed(json.as_bytes())?.0)
    }

    /// Load from either wire format (JSON or binary), detected by content.
    pub fn from_bytes(bytes: &[u8]) -> crate::Result<Document> {
        Ok(Self::load_timed(bytes)?.0)
    }

    /// Load from either wire format, timing each stage separately.
    pub fn load_timed(bytes: &[u8]) -> crate::Result<(Document, LoadTimings)> {
        let mut t = LoadTimings::default();
        let clock = web_time::Instant::now();
        let dast = crate::dast::load(bytes)?;
        t.deserialize = clock.elapsed();

        let clock = web_time::Instant::now();
        let unscheduled = crate::build::build(&dast)?;
        t.build = clock.elapsed();

        let clock = web_time::Instant::now();
        let mut doc = unscheduled.schedule()?;
        t.schedule = clock.elapsed();

        let clock = web_time::Instant::now();
        doc.recompute();
        t.initial_compute = clock.elapsed();
        Ok((doc, t))
    }

    pub fn from_dast(dast: &crate::dast::Dast) -> crate::Result<Document> {
        let mut doc = crate::build::build(dast)?.schedule()?;
        doc.recompute();
        Ok(doc)
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
            match self.program.invert_to_essential(&self.cells, r.cell, r.value) {
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
        }
        tick
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

    pub fn component(&self, name: &str) -> Option<CompIdx> {
        self.name_map
            .get_or_init(|| (0..self.comps.len() as CompIdx).filter_map(|c| self.name(c).map(|n| (n.to_string(), c))).collect())
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
            program: self.program.instrs.capacity() * std::mem::size_of::<crate::ops::Instr>() + self.program.producer.capacity() * 4,
            components: self.comps.heap_bytes(),
            strings: self.strings.heap_bytes(),
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
}

impl MemoryEstimate {
    pub fn total(&self) -> usize {
        self.cells + self.program + self.components + self.strings
    }
}
