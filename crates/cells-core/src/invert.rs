//! The inversion engine: requests are gathered per producing instruction,
//! highest schedule position first, and inverted once; point groups are
//! kept together through lookahead (ADR 0006).

use std::collections::{BinaryHeap, HashMap, HashSet};

use crate::document::{CellIdx, Request};
use crate::geo::{PointWrite, Produced};
use crate::ops::Op;
use crate::program::Program;

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

/// The essential writes a set of requests resolves to.
#[derive(Debug, Clone, Default)]
pub struct Inversion {
    /// (essential cell, value), in resolution order, one entry per cell
    /// (a later write to the same cell replaces the earlier one in place).
    pub writes: Vec<(CellIdx, f64)>,
    /// Requests that could not be inverted or landed on a fixed cell.
    pub dropped: Vec<Request>,
}

impl Program {
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
                Op::Vec(v, start) => {
                    let inputs = &self.program.extra[start as usize..start as usize + v.n_in()];
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
