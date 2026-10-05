//! Recompute strategies. All take the changed essential cells as seeds and
//! append the derived cells whose value changed. They must agree on results;
//! they differ only in how much work they do.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use crate::document::CellIdx;
use crate::program::Program;

pub trait Evaluator {
    fn name(&self) -> &'static str;
    /// `changed` holds the essential cells written this tick on entry.
    fn recompute(&mut self, program: &Program, cells: &mut [f64], changed: &mut Vec<CellIdx>);
}

/// Run every instruction in schedule order, ignoring the seeds.
#[derive(Default)]
pub struct FullRecompute;

impl Evaluator for FullRecompute {
    fn name(&self) -> &'static str {
        "full"
    }
    fn recompute(&mut self, program: &Program, cells: &mut [f64], changed: &mut Vec<CellIdx>) {
        program.run_all_tracking(cells, changed);
    }
}

/// Scan every instruction in schedule order but evaluate only those with a
/// dirty input. Linear in the program, no evaluation of clean cells.
pub struct DirtyScan {
    dirty: Vec<bool>,
}

impl DirtyScan {
    pub fn new(n_cells: usize) -> Self {
        DirtyScan { dirty: vec![false; n_cells] }
    }
}

impl Evaluator for DirtyScan {
    fn name(&self) -> &'static str {
        "dirty-scan"
    }
    fn recompute(&mut self, program: &Program, cells: &mut [f64], changed: &mut Vec<CellIdx>) {
        for &c in changed.iter() {
            self.dirty[c as usize] = true;
        }
        for ins in &program.instrs {
            if ins.op.inputs(&program.extra).any(|c| self.dirty[c as usize]) {
                let before = changed.len();
                program.step(ins, cells, Some(changed));
                for &c in &changed[before..] {
                    self.dirty[c as usize] = true;
                }
            }
        }
        for &c in changed.iter() {
            self.dirty[c as usize] = false;
        }
    }
}

/// Walk only the downstream closure of the seeds, in schedule order, using a
/// min-heap over instruction positions. Work is proportional to the closure.
pub struct DirtyClosure {
    /// dependents[cell] = schedule positions of instructions reading it
    dependents: Vec<Vec<u32>>,
    queued: Vec<bool>,
    heap: BinaryHeap<Reverse<u32>>,
}

impl DirtyClosure {
    pub fn new(program: &Program, n_cells: usize) -> Self {
        let mut dependents = vec![Vec::new(); n_cells];
        for (i, ins) in program.instrs.iter().enumerate() {
            for input in ins.op.inputs(&program.extra) {
                dependents[input as usize].push(i as u32);
            }
        }
        DirtyClosure { dependents, queued: vec![false; program.len()], heap: BinaryHeap::new() }
    }

    #[inline]
    fn enqueue_dependents(&mut self, cell: CellIdx) {
        for &i in &self.dependents[cell as usize] {
            if !self.queued[i as usize] {
                self.queued[i as usize] = true;
                self.heap.push(Reverse(i));
            }
        }
    }
}

impl Evaluator for DirtyClosure {
    fn name(&self) -> &'static str {
        "dirty-closure"
    }
    fn recompute(&mut self, program: &Program, cells: &mut [f64], changed: &mut Vec<CellIdx>) {
        let seeds: Vec<CellIdx> = changed.clone();
        for c in seeds {
            self.enqueue_dependents(c);
        }
        while let Some(Reverse(i)) = self.heap.pop() {
            self.queued[i as usize] = false;
            let ins = &program.instrs[i as usize];
            let before = changed.len();
            program.step(ins, cells, Some(changed));
            for k in before..changed.len() {
                let c = changed[k];
                self.enqueue_dependents(c);
            }
        }
    }
}
