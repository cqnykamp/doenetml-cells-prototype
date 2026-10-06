//! The component table as a renderer or a test reads it: kinds, names,
//! children, prop cells, and resolution of dotted paths to components.

use super::*;

impl Document {
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
        self.comps.child_list[s..s + n].iter().map(move |&e| if e & TEXT_BIT != 0 { Child::Text(self.strings.get(e & !TEXT_BIT)) } else { Child::Component(e) })
    }

    /// Cells of the single-cell props of `c`, in `kind.prop_defs()` order.
    pub fn comp_cells(&self, c: CompIdx) -> &[CellIdx] {
        let base = self.comps.prop_base[c as usize] as usize;
        &self.comps.prop_cells[base..base + self.kind(c).prop_defs().len()]
    }

    /// The component a path names; see `resolve_path`.
    pub fn component(&self, path: &str) -> Option<CompIdx> {
        self.resolve_path(path)
    }

    pub fn component_names(&self) -> impl Iterator<Item = (&str, CompIdx)> {
        (0..self.comps.len() as CompIdx).filter_map(|c| self.name(c).map(|n| (n, c)))
    }

    /// Cells of a prop, including virtual props such as a point's `coords`
    /// and array props such as a line's `points` (items flattened, live
    /// items only).
    pub fn prop_cells(&self, comp: CompIdx, prop: &str) -> Option<Vec<CellIdx>> {
        let kind = self.kind(comp);
        if let Some(parts) = kind.virtual_prop(prop) {
            return parts.iter().map(|p| self.prop_cells(comp, p).map(|v| v[0])).collect();
        }
        if let Some(items) = kind.array_prop(prop) {
            let live = match kind {
                ComponentKind::Polygon => self.cells[self.comp_cells(comp)[0] as usize] as usize,
                _ => items.len(),
            };
            return items.iter().take(live).map(|[x, y]| Some([self.prop_cells(comp, x)?[0], self.prop_cells(comp, y)?[0]])).collect::<Option<Vec<_>>>().map(|v| v.concat());
        }
        let i = kind.prop_index(prop)?;
        Some(vec![self.comp_cells(comp)[i]])
    }

    /// The single cell behind `path.prop`.
    pub fn cell(&self, path: &str, prop: &str) -> Option<CellIdx> {
        let cells = self.prop_cells(self.resolve_path(path)?, prop)?;
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
