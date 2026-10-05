//! The instruction list and its topological schedule.

use crate::document::CellIdx;
use crate::expr::Arena;
use crate::ops::Instr;

/// Instructions in a valid evaluation order: every instruction's inputs are
/// either essential cells or outputs of earlier instructions.
#[derive(Debug, Clone, Default)]
pub struct Program {
    pub instrs: Vec<Instr>,
    /// producer[cell] = index into `instrs` of the instruction writing that
    /// cell, or `u32::MAX` for essential and fixed cells.
    pub producer: Vec<u32>,
    /// Symbolic expressions that `Evaluate`/`EvalAt` instructions read.
    pub arena: Arena,
    /// Extra inputs of `Evaluate`/`EvalAt` instructions (expression cell leaves).
    pub extra: Vec<CellIdx>,
    /// Whether creation order was already a valid evaluation order, so no
    /// sort ran. Diagnostic.
    pub in_creation_order: bool,
}

impl Program {
    /// Orders `instrs` topologically. Returns the cell index of an output
    /// involved in a cycle on failure.
    ///
    /// Fast path: the builder emits instructions in creation order, which
    /// for a template stamped per iteration is almost always already a valid
    /// evaluation order (within an iteration the template is in document
    /// order; a lag of `k - d` reads an earlier iteration). One linear pass
    /// checks that every input is essential, fixed, or produced earlier; only
    /// when that fails does the general sort run.
    pub fn schedule(instrs: Vec<Instr>, n_cells: usize, arena: Arena, extra: Vec<CellIdx>) -> std::result::Result<Program, CellIdx> {
        let mut producer = vec![u32::MAX; n_cells];
        for (i, ins) in instrs.iter().enumerate() {
            debug_assert_eq!(producer[ins.out as usize], u32::MAX, "two instructions write one cell");
            producer[ins.out as usize] = i as u32;
        }
        let in_order = instrs.iter().enumerate().all(|(i, ins)| ins.op.inputs(&extra).all(|input| {
            let p = producer[input as usize];
            p == u32::MAX || (p as usize) < i
        }));
        if in_order {
            return Ok(Program { instrs, producer, arena, extra, in_creation_order: true });
        }

        // Kahn's algorithm over instructions, with the dependents lists in
        // compressed-sparse-row form (two counting passes, no per-instruction
        // allocation).
        let n = instrs.len();
        let mut indegree = vec![0u32; n];
        let mut dep_count = vec![0u32; n + 1];
        for ins in instrs.iter() {
            for input in ins.op.inputs(&extra) {
                let p = producer[input as usize];
                if p != u32::MAX {
                    dep_count[p as usize + 1] += 1;
                }
            }
        }
        for i in 0..n {
            dep_count[i + 1] += dep_count[i];
        }
        let mut fill = dep_count.clone();
        let mut dependents = vec![0u32; dep_count[n] as usize];
        for (i, ins) in instrs.iter().enumerate() {
            for input in ins.op.inputs(&extra) {
                let p = producer[input as usize];
                if p != u32::MAX {
                    indegree[i] += 1;
                    dependents[fill[p as usize] as usize] = i as u32;
                    fill[p as usize] += 1;
                }
            }
        }
        let mut ready: Vec<u32> = (0..n as u32).filter(|&i| indegree[i as usize] == 0).collect();
        ready.reverse();
        let mut order = Vec::with_capacity(n);
        while let Some(i) = ready.pop() {
            order.push(instrs[i as usize]);
            for &d in &dependents[dep_count[i as usize] as usize..dep_count[i as usize + 1] as usize] {
                indegree[d as usize] -= 1;
                if indegree[d as usize] == 0 {
                    ready.push(d);
                }
            }
        }
        if order.len() != n {
            let stuck = (0..n).find(|&i| indegree[i] > 0).unwrap();
            return Err(instrs[stuck].out);
        }
        let mut producer = vec![u32::MAX; n_cells];
        for (i, ins) in order.iter().enumerate() {
            producer[ins.out as usize] = i as u32;
        }
        Ok(Program { instrs: order, producer, arena, extra, in_creation_order: false })
    }

    /// Recompute every derived cell, appending the indices whose value
    /// changed to `changed`. NaN to NaN counts as unchanged.
    #[inline]
    pub fn run_all_tracking(&self, cells: &mut [f64], changed: &mut Vec<CellIdx>) {
        for ins in &self.instrs {
            let out = ins.out as usize;
            let new = ins.op.eval(cells, &self.arena);
            let old = cells[out];
            if new != old && !(new.is_nan() && old.is_nan()) {
                cells[out] = new;
                changed.push(ins.out);
            }
        }
    }

    /// Resolve a request on `cell` down to the essential cell that receives
    /// the write, or `None` if some inverse is undefined.
    #[inline]
    pub fn invert_to_essential(&self, cells: &[f64], mut cell: CellIdx, mut value: f64) -> Option<(CellIdx, f64)> {
        loop {
            let p = self.producer[cell as usize];
            if p == u32::MAX {
                return Some((cell, value));
            }
            (cell, value) = self.instrs[p as usize].op.invert(cells, value)?;
        }
    }

    /// Recompute every derived cell in schedule order.
    #[inline]
    pub fn run_all(&self, cells: &mut [f64]) {
        for ins in &self.instrs {
            cells[ins.out as usize] = ins.op.eval(cells, &self.arena);
        }
    }

    pub fn len(&self) -> usize {
        self.instrs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.instrs.is_empty()
    }
}
