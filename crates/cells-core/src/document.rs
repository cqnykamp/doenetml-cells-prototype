//! The loaded document: the cell array, the program that derives cells, and
//! the component naming layer over cell indices.

use std::collections::HashMap;
use std::time::Duration;

use crate::components::ComponentKind;
use crate::program::Program;

pub type CellIdx = u32;
pub type CompIdx = u32;

#[derive(Debug, Clone)]
pub struct Prop {
    pub name: &'static str,
    pub cells: Vec<CellIdx>,
}

#[derive(Debug, Clone)]
pub enum Child {
    Component(CompIdx),
    Text(String),
}

#[derive(Debug, Clone)]
pub struct Component {
    pub kind: ComponentKind,
    pub name: Option<String>,
    pub parent: Option<CompIdx>,
    pub children: Vec<Child>,
    /// Single-cell props in `kind.prop_defs()` order.
    pub props: Vec<Prop>,
}

#[derive(Debug, Clone)]
pub struct Document {
    /// All cells. Essential cells come first, then derived cells.
    pub cells: Vec<f64>,
    /// Number of essential cells; `cells[..n_essential]` are essential.
    pub n_essential: usize,
    pub program: Program,
    pub components: Vec<Component>,
    /// Index of the root `<document>` component.
    pub root: CompIdx,
    names: HashMap<String, CompIdx>,
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
    pub(crate) fn new(
        cells: Vec<f64>,
        n_essential: usize,
        program: Program,
        components: Vec<Component>,
        root: CompIdx,
        names: HashMap<String, CompIdx>,
    ) -> Self {
        Document { cells, n_essential, program, components, root, names }
    }

    /// Load from DAST JSON and compute initial values.
    pub fn from_dast_json(json: &str) -> crate::Result<Document> {
        Ok(Self::load_timed(json)?.0)
    }

    /// Load from DAST JSON, timing each stage separately.
    pub fn load_timed(json: &str) -> crate::Result<(Document, LoadTimings)> {
        let mut t = LoadTimings::default();
        let clock = std::time::Instant::now();
        let dast = crate::dast::parse_json(json)?;
        t.deserialize = clock.elapsed();

        let clock = std::time::Instant::now();
        let unscheduled = crate::build::build(&dast)?;
        t.build = clock.elapsed();

        let clock = std::time::Instant::now();
        let mut doc = unscheduled.schedule()?;
        t.schedule = clock.elapsed();

        let clock = std::time::Instant::now();
        doc.recompute();
        t.initial_compute = clock.elapsed();
        Ok((doc, t))
    }

    pub fn from_dast(dast: &crate::dast::DastRoot) -> crate::Result<Document> {
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

    pub fn component(&self, name: &str) -> Option<CompIdx> {
        self.names.get(name).copied()
    }

    pub fn component_names(&self) -> impl Iterator<Item = (&str, CompIdx)> {
        self.names.iter().map(|(k, v)| (k.as_str(), *v))
    }

    /// Cells of a prop, including virtual props such as a point's `coords`.
    pub fn prop_cells(&self, comp: CompIdx, prop: &str) -> Option<Vec<CellIdx>> {
        let c = &self.components[comp as usize];
        if let Some(parts) = c.kind.virtual_prop(prop) {
            return parts.iter().map(|p| self.prop_cells(comp, p).map(|v| v[0])).collect();
        }
        let i = c.kind.prop_index(prop)?;
        Some(c.props[i].cells.clone())
    }

    /// The single cell behind `name.prop`.
    pub fn cell(&self, name: &str, prop: &str) -> Option<CellIdx> {
        let cells = self.prop_cells(self.component(name)?, prop)?;
        (cells.len() == 1).then(|| cells[0])
    }

    pub fn value(&self, name: &str, prop: &str) -> Option<f64> {
        self.cell(name, prop).map(|c| self.cells[c as usize])
    }
}

/// Rough heap footprint of a loaded document, by part.
#[derive(Debug, Clone, Copy, Default)]
pub struct MemoryEstimate {
    pub cells: usize,
    pub program: usize,
    pub components: usize,
}

impl MemoryEstimate {
    pub fn total(&self) -> usize {
        self.cells + self.program + self.components
    }
}

impl Document {
    pub fn memory_estimate(&self) -> MemoryEstimate {
        use std::mem::size_of;
        let components = self
            .components
            .iter()
            .map(|c| {
                size_of::<Component>()
                    + c.name.as_ref().map_or(0, |n| n.capacity())
                    + c.children.iter().map(|ch| size_of::<Child>() + if let Child::Text(t) = ch { t.capacity() } else { 0 }).sum::<usize>()
                    + c.props.iter().map(|p| size_of::<Prop>() + p.cells.capacity() * size_of::<CellIdx>()).sum::<usize>()
            })
            .sum::<usize>()
            + self.names.keys().map(|k| k.capacity() + size_of::<(String, CompIdx)>()).sum::<usize>();
        MemoryEstimate {
            cells: self.cells.capacity() * size_of::<f64>(),
            program: self.program.instrs.capacity() * size_of::<crate::ops::Instr>() + self.program.producer.capacity() * size_of::<u32>(),
            components,
        }
    }
}
