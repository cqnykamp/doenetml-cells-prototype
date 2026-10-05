//! The instruction list, its topological schedule, and the inversion engine.

use std::collections::{BinaryHeap, HashMap, HashSet};

use crate::document::{CellIdx, Request};
use crate::expr::Arena;
use crate::geo::{PointWrite, Produced};
use crate::ops::{Instr, Op};

/// A request on both cells of a point, issued together with others as a
/// point group: the renderer's whole-shape drag, or the points one inverse
/// moves at once.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointRequest {
    pub cells: [CellIdx; 2],
    pub values: [f64; 2],
}

/// Tolerance for "this point landed where it was asked to".
const REALIZED_TOL: f64 = 1e-6;

/// Instructions in a valid evaluation order: every instruction's inputs are
/// either essential cells or outputs of earlier instructions.
#[derive(Debug, Clone, Default)]
pub struct Program {
    pub instrs: Vec<Instr>,
    /// producer[cell] = index into `instrs` of the instruction writing that
    /// cell, or `u32::MAX` for essential and fixed cells. A vector
    /// instruction produces several consecutive cells.
    pub producer: Vec<u32>,
    /// Symbolic expressions that `Evaluate`/`EvalAt` instructions read.
    pub arena: Arena,
    /// Extra inputs: expression cell leaves of `Evaluate`/`EvalAt`, and every
    /// input of a vector instruction.
    pub extra: Vec<CellIdx>,
    /// Whether creation order was already a valid evaluation order, so no
    /// sort ran. Diagnostic.
    pub in_creation_order: bool,
}

/// The essential writes a set of requests resolves to.
#[derive(Debug, Clone, Default)]
pub struct Inversion {
    /// (essential cell, value), in resolution order, one entry per cell
    /// (a later write to the same cell replaces the earlier one in place).
    pub writes: Vec<(CellIdx, f64)>,
    /// Requests that could not be inverted or landed on a fixed cell.
    pub dropped: Vec<Request>,
}

#[inline(always)]
fn differs(new: f64, old: f64) -> bool {
    new != old && !(new.is_nan() && old.is_nan())
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
            for k in 0..ins.op.n_out() {
                let out = ins.out as usize + k;
                debug_assert_eq!(producer[out], u32::MAX, "two instructions write one cell");
                producer[out] = i as u32;
            }
        }
        let in_order = instrs.iter().enumerate().all(|(i, ins)| {
            ins.op.inputs(&extra).all(|input| {
                let p = producer[input as usize];
                p == u32::MAX || (p as usize) < i
            })
        });
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
            for k in 0..ins.op.n_out() {
                producer[ins.out as usize + k] = i as u32;
            }
        }
        Ok(Program { instrs: order, producer, arena, extra, in_creation_order: false })
    }

    /// Evaluate one instruction into `cells`. With `changed`, outputs whose
    /// value changed are appended (NaN to NaN counts as unchanged).
    #[inline(always)]
    pub fn step(&self, ins: &Instr, cells: &mut [f64], changed: Option<&mut Vec<CellIdx>>) {
        let out = ins.out as usize;
        if let Op::Vec(..) = ins.op {
            let mut buf = [0.0f64; 16];
            let n = ins.op.n_out();
            ins.op.eval_vec(cells, &self.extra, &mut buf[..n]);
            match changed {
                Some(changed) => {
                    for k in 0..n {
                        if differs(buf[k], cells[out + k]) {
                            cells[out + k] = buf[k];
                            changed.push((out + k) as CellIdx);
                        }
                    }
                }
                None => cells[out..out + n].copy_from_slice(&buf[..n]),
            }
        } else {
            let new = ins.op.eval(cells, &self.arena);
            match changed {
                Some(changed) => {
                    if differs(new, cells[out]) {
                        cells[out] = new;
                        changed.push(ins.out);
                    }
                }
                None => cells[out] = new,
            }
        }
    }

    /// Recompute every derived cell, appending the indices whose value
    /// changed to `changed`. NaN to NaN counts as unchanged.
    #[inline]
    pub fn run_all_tracking(&self, cells: &mut [f64], changed: &mut Vec<CellIdx>) {
        for ins in &self.instrs {
            self.step(ins, cells, Some(changed));
        }
    }

    /// Recompute every derived cell in schedule order.
    #[inline]
    pub fn run_all(&self, cells: &mut [f64]) {
        for ins in &self.instrs {
            self.step(ins, cells, None);
        }
    }

    /// Whether any input of `ins` differs between two cell arrays.
    #[inline]
    pub fn any_input_differs(&self, ins: &Instr, a: &[f64], b: &[f64]) -> bool {
        ins.op.inputs(&self.extra).any(|c| differs(a[c as usize], b[c as usize]))
    }

    /// Resolve a request on `cell` down to the essential cell that receives
    /// the write, or `None` if some inverse is undefined. Scalar chains only;
    /// `invert_requests` is the general engine.
    #[inline]
    pub fn invert_to_essential(&self, cells: &[f64], mut cell: CellIdx, mut value: f64) -> Option<(CellIdx, f64)> {
        loop {
            let p = self.producer[cell as usize];
            if p == u32::MAX {
                return Some((cell, value));
            }
            let ins = &self.instrs[p as usize];
            if let Op::Vec(..) = ins.op {
                let inv = self.invert_requests(cells, usize::MAX, &[Request { cell, value }], &[]);
                return match inv.writes.as_slice() {
                    [w] if inv.dropped.is_empty() => Some(*w),
                    _ => None,
                };
            }
            (cell, value) = ins.op.invert(cells, value)?;
        }
    }

    /// Resolve requests to essential writes. Requests are gathered per
    /// producing instruction, highest schedule position first, so every
    /// request on an instruction's outputs is known before it is inverted
    /// once; the input requests an inverse produces join the queue. Cells
    /// below `n_essential` are essential; a request landing on another
    /// producer-less cell (a fixed cell) is dropped.
    ///
    /// `groups` are points requested together (ADR 0006): before such a
    /// group is queued the engine asks what each point would actually
    /// become, and if a strict subset is held back, all by the same shift,
    /// the shift is applied to the rest, so a shape dragged against a
    /// constrained point keeps its shape. The same happens to the point
    /// groups an inverse produces.
    pub fn invert_requests(&self, cells: &[f64], n_essential: usize, requests: &[Request], groups: &[Vec<PointRequest>]) -> Inversion {
        let mut engine =
            Engine { program: self, cells, n_essential, pending: HashMap::new(), heap: BinaryHeap::new(), queued: HashSet::new(), write_index: HashMap::new(), inversion: Inversion::default(), origin: HashMap::new() };
        for &r in requests {
            engine.push(r.cell, r.value, r);
        }
        for g in groups {
            let pts: Vec<PointWrite> = g.iter().map(|p| [(p.cells[0], p.values[0]), (p.cells[1], p.values[1])]).collect();
            let origin = g.first().map(|p| Request { cell: p.cells[0], value: p.values[0] }).unwrap_or(Request { cell: 0, value: f64::NAN });
            engine.push_group(pts, origin);
        }
        engine.run();
        engine.inversion
    }

    /// What `cells` would hold for each requested cell if the requests were
    /// applied: invert, overlay the essential writes on a copy, and evaluate
    /// forward the instructions that read a changed value, up to the last
    /// requested cell. Nothing is written.
    pub fn realize(&self, cells: &[f64], n_essential: usize, requests: &[(CellIdx, f64)], out: &mut Vec<f64>) {
        let reqs: Vec<Request> = requests.iter().map(|&(cell, value)| Request { cell, value }).collect();
        let inv = self.invert_requests(cells, n_essential, &reqs, &[]);
        let mut scratch = cells.to_vec();
        for &(c, v) in &inv.writes {
            scratch[c as usize] = v;
        }
        let hi = requests.iter().map(|&(c, _)| self.producer[c as usize]).filter(|&p| p != u32::MAX).max();
        if let Some(hi) = hi {
            for ins in &self.instrs[..=hi as usize] {
                if self.any_input_differs(ins, &scratch, cells) {
                    self.step(ins, &mut scratch, None);
                }
            }
        }
        out.extend(requests.iter().map(|&(c, _)| scratch[c as usize]));
    }

    pub fn len(&self) -> usize {
        self.instrs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.instrs.is_empty()
    }
}

/// One run of the inversion engine.
struct Engine<'a> {
    program: &'a Program,
    cells: &'a [f64],
    n_essential: usize,
    /// Requested value per derived cell awaiting its producer's inverse.
    pending: HashMap<CellIdx, f64>,
    /// Producers with pending requests, highest schedule position first.
    heap: BinaryHeap<u32>,
    queued: HashSet<u32>,
    /// Position in `inversion.writes` of each essential cell written.
    write_index: HashMap<CellIdx, usize>,
    inversion: Inversion,
    /// The renderer's request each pending cell descends from, for the
    /// dropped list.
    origin: HashMap<CellIdx, Request>,
}

impl Engine<'_> {
    fn push(&mut self, cell: CellIdx, value: f64, origin: Request) {
        let p = self.program.producer[cell as usize];
        if p == u32::MAX {
            if (cell as usize) < self.n_essential {
                match self.write_index.get(&cell) {
                    Some(&i) => self.inversion.writes[i].1 = value,
                    None => {
                        self.write_index.insert(cell, self.inversion.writes.len());
                        self.inversion.writes.push((cell, value));
                    }
                }
            } else {
                self.inversion.dropped.push(origin);
            }
            return;
        }
        self.pending.insert(cell, value);
        self.origin.insert(cell, origin);
        if self.queued.insert(p) {
            self.heap.push(p);
        }
    }

    /// Queue a point group after the equal-shift rule (see `invert_requests`).
    fn push_group(&mut self, mut pts: Vec<PointWrite>, origin: Request) {
        if pts.len() >= 2 {
            let reqs: Vec<(CellIdx, f64)> = pts.iter().flatten().copied().collect();
            let mut realized = Vec::with_capacity(reqs.len());
            self.program.realize(self.cells, self.n_essential, &reqs, &mut realized);
            let mut shift: Option<(f64, f64)> = None;
            let mut n_held = 0;
            let mut consistent = true;
            for (i, p) in pts.iter().enumerate() {
                let (sx, sy) = (realized[2 * i] - p[0].1, realized[2 * i + 1] - p[1].1);
                if sx.abs() > REALIZED_TOL || sy.abs() > REALIZED_TOL {
                    n_held += 1;
                    match shift {
                        None => shift = Some((sx, sy)),
                        Some((px, py)) if (px - sx).abs() <= REALIZED_TOL && (py - sy).abs() <= REALIZED_TOL => {}
                        Some(_) => consistent = false,
                    }
                }
            }
            if let Some((sx, sy)) = shift {
                if consistent && n_held < pts.len() && sx.is_finite() && sy.is_finite() {
                    for p in &mut pts {
                        p[0].1 += sx;
                        p[1].1 += sy;
                    }
                }
            }
        }
        for p in pts {
            self.push(p[0].0, p[0].1, origin);
            self.push(p[1].0, p[1].1, origin);
        }
    }

    fn run(&mut self) {
        let mut desired: Vec<Option<f64>> = Vec::with_capacity(16);
        let mut produced = Produced::default();
        while let Some(p) = self.heap.pop() {
            self.queued.remove(&p);
            let ins = &self.program.instrs[p as usize];
            let n_out = ins.op.n_out();
            desired.clear();
            let mut origin = None;
            for k in 0..n_out {
                let c = ins.out + k as CellIdx;
                let d = self.pending.remove(&c);
                if d.is_some() {
                    origin = origin.or_else(|| self.origin.remove(&c));
                }
                desired.push(d);
            }
            let origin = origin.unwrap_or(Request { cell: ins.out, value: f64::NAN });
            produced.clear();
            let ok = match ins.op {
                Op::Vec(v, start, n_in, _) => {
                    let inputs = &self.program.extra[start as usize..start as usize + n_in as usize];
                    let inp: Vec<f64> = inputs.iter().map(|&c| self.cells[c as usize]).collect();
                    let cur: Vec<f64> = (0..n_out).map(|k| self.cells[ins.out as usize + k]).collect();
                    v.invert(inputs, &inp, &cur, &desired, &mut produced)
                }
                _ => match ins.op.invert(self.cells, desired[0].unwrap()) {
                    Some((c, v)) => {
                        produced.write(c, v);
                        true
                    }
                    None => false,
                },
            };
            if !ok {
                self.inversion.dropped.push(origin);
                continue;
            }
            for &(c, v) in &produced.writes {
                self.push(c, v, origin);
            }
            let groups = std::mem::take(&mut produced.groups);
            for g in groups {
                self.push_group(g, origin);
            }
        }
    }
}
