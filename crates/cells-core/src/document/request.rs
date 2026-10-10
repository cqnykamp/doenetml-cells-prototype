//! The request entry points: one tick, from a renderer's requests to the
//! cells that changed. The stages themselves are in `tick/`: the sticky
//! pre-pass (`sticky.rs`, here because it reads the component table), then
//! inversion (`tick/invert.rs`) and recompute (`tick/eval.rs`), then a
//! rebuild if a structural cell changed (`load.rs`).

use super::Document;
use crate::tick::eval::{Evaluator, FullRecompute};
use crate::tick::invert::{PointRequest, invert_requests};
use crate::tick::{Request, TickOutcome};

impl Document {
    /// Apply requests: invert each to an essential cell (later requests win
    /// when two land on one cell), recompute, and report what changed.
    pub fn request(&mut self, requests: &[Request]) -> TickOutcome {
        self.request_with_groups(&mut FullRecompute, requests, &[])
    }

    /// Apply a point group: points dragged together, which keep their shape
    /// when one of them is constrained (ADR 0006).
    pub fn request_points(&mut self, points: &[PointRequest]) -> TickOutcome {
        self.request_with_groups(&mut FullRecompute, &[], &[points.to_vec()])
    }

    /// Scalar requests and point groups in one tick.
    pub fn request_with_groups(
        &mut self,
        evaluator: &mut (impl Evaluator + ?Sized),
        requests: &[Request],
        groups: &[Vec<PointRequest>],
    ) -> TickOutcome {
        let mut tick = TickOutcome::default();
        // An infinite ask is never meaningful state (NaN is: an emptied
        // input), and the current core rejects it; drop it before inverting.
        let (mut finite, infinite): (Vec<Request>, Vec<Request>) = requests
            .iter()
            .partition(|r| !r.value.is_infinite() || self.accepts_infinity(r.cell));
        tick.dropped.extend(infinite);
        let mut finite_groups: Vec<Vec<PointRequest>> = Vec::with_capacity(groups.len());
        for g in groups {
            if g.iter().any(|p| p.values.iter().any(|v| v.is_infinite())) {
                tick.dropped.extend(g.iter().map(|p| Request {
                    cell: p.cells[0],
                    value: p.values[0],
                }));
            } else {
                finite_groups.push(g.clone());
            }
        }
        self.snap_sticky(&mut finite, &mut finite_groups);
        let inv = invert_requests(
            &self.program,
            &self.cells,
            self.n_essential,
            &finite,
            &finite_groups,
        );
        tick.dropped.extend(inv.dropped);
        for (cell, value) in inv.writes {
            let old = self.cells[cell as usize];
            if value != old && !(value.is_nan() && old.is_nan()) {
                self.cells[cell as usize] = value;
                tick.changed.push(cell);
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
}
