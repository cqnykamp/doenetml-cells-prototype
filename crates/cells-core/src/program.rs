//! The instruction list and its topological schedule.

use crate::document::CellIdx;
use crate::ops::Instr;

/// Instructions in a valid evaluation order: every instruction's inputs are
/// either essential cells or outputs of earlier instructions.
#[derive(Debug, Clone, Default)]
pub struct Program {
    pub instrs: Vec<Instr>,
}

impl Program {
    /// Orders `instrs` topologically. Returns the cell index of an output
    /// involved in a cycle on failure.
    pub fn schedule(instrs: Vec<Instr>, n_cells: usize) -> std::result::Result<Program, CellIdx> {
        // producer[cell] = index of the instruction that writes it, if any
        let mut producer = vec![u32::MAX; n_cells];
        for (i, ins) in instrs.iter().enumerate() {
            debug_assert_eq!(producer[ins.out as usize], u32::MAX, "two instructions write one cell");
            producer[ins.out as usize] = i as u32;
        }
        // Kahn's algorithm over instructions.
        let n = instrs.len();
        let mut indegree = vec![0u32; n];
        let mut dependents: Vec<Vec<u32>> = vec![Vec::new(); n];
        for (i, ins) in instrs.iter().enumerate() {
            for input in ins.op.inputs() {
                let p = producer[input as usize];
                if p != u32::MAX {
                    indegree[i] += 1;
                    dependents[p as usize].push(i as u32);
                }
            }
        }
        let mut ready: Vec<u32> = (0..n as u32).filter(|&i| indegree[i as usize] == 0).collect();
        ready.reverse();
        let mut order = Vec::with_capacity(n);
        while let Some(i) = ready.pop() {
            order.push(instrs[i as usize]);
            for &d in &dependents[i as usize] {
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
        Ok(Program { instrs: order })
    }

    /// Recompute every derived cell in schedule order.
    #[inline]
    pub fn run_all(&self, cells: &mut [f64]) {
        for ins in &self.instrs {
            cells[ins.out as usize] = ins.op.eval(cells);
        }
    }

    pub fn len(&self) -> usize {
        self.instrs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.instrs.is_empty()
    }
}
