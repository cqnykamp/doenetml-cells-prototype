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

    /// Write an essential cell and recompute. Panics if `cell` is derived;
    /// requests on derived cells arrive with inversion in a later milestone.
    pub fn set_essential(&mut self, cell: CellIdx, value: f64) {
        assert!(self.is_essential(cell), "cell {cell} is derived");
        self.cells[cell as usize] = value;
        self.recompute();
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
