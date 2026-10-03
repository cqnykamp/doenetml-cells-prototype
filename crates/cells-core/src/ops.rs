//! Operators: the numeric functions that compute derived cells.
//! Literal parameters are stored inline in the instruction, not in cells.
//!
//! Inverse rules (see ADR 0003): binary operators write their first
//! argument (except `Default`, which writes whichever argument it is
//! currently passing through); idempotent operators (Round, Floor, Clamp, Min, Max) invert by
//! projection, applying themselves to the desired value; an inverse that is
//! undefined for the current values, or whose desired value is outside the
//! operator's domain (a non-finite ask on Round), returns `None` and the
//! request is dropped.

use crate::document::CellIdx;
use crate::expr::Arena;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Op {
    Add(CellIdx, CellIdx),
    Sub(CellIdx, CellIdx),
    Mul(CellIdx, CellIdx),
    Div(CellIdx, CellIdx),
    /// NaN if either argument is NaN (unlike `f64::min`).
    Min(CellIdx, CellIdx),
    /// NaN if either argument is NaN (unlike `f64::max`).
    Max(CellIdx, CellIdx),
    /// `a` unless it is NaN, then `b`. The cell-level counterpart of the
    /// current core's `valueOnNaN`; it is what gives a recurrence its seed
    /// where a lagged reference has no referent.
    Default(CellIdx, CellIdx),
    Negate(CellIdx),
    /// Round half away from zero, as `f64::round`.
    Round(CellIdx),
    Floor(CellIdx),
    Scale(CellIdx, f64),
    Offset(CellIdx, f64),
    Clamp(CellIdx, f64, f64),
    /// `a + t * (b - a)`
    Lerp(CellIdx, CellIdx, f64),
    Pow(CellIdx, CellIdx),
    /// Evaluate the expression whose arena handle the cell holds; NaN while
    /// it has free symbols. The expression's cell leaves are the
    /// instruction's extra inputs `extra[start..start + n]` (see
    /// `Program::extra`), so scheduling and dirty tracking see them.
    Evaluate(CellIdx, u32, u8),
    /// Evaluate the expression in the first cell with its free symbol set to
    /// the second cell's value; extra inputs as for `Evaluate`.
    EvalAt(CellIdx, CellIdx, u32, u8),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Instr {
    pub out: CellIdx,
    pub op: Op,
}

#[inline(always)]
fn nan_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() { f64::NAN } else { a.min(b) }
}

#[inline(always)]
fn nan_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() { f64::NAN } else { a.max(b) }
}

impl Op {
    #[inline(always)]
    pub fn eval(&self, cells: &[f64], arena: &Arena) -> f64 {
        match *self {
            Op::Add(a, b) => cells[a as usize] + cells[b as usize],
            Op::Sub(a, b) => cells[a as usize] - cells[b as usize],
            Op::Mul(a, b) => cells[a as usize] * cells[b as usize],
            Op::Div(a, b) => cells[a as usize] / cells[b as usize],
            Op::Min(a, b) => nan_min(cells[a as usize], cells[b as usize]),
            Op::Max(a, b) => nan_max(cells[a as usize], cells[b as usize]),
            Op::Default(a, b) => {
                let x = cells[a as usize];
                if x.is_nan() { cells[b as usize] } else { x }
            }
            Op::Negate(a) => -cells[a as usize],
            Op::Round(a) => cells[a as usize].round(),
            Op::Floor(a) => cells[a as usize].floor(),
            Op::Scale(a, k) => cells[a as usize] * k,
            Op::Offset(a, k) => cells[a as usize] + k,
            Op::Clamp(a, lo, hi) => cells[a as usize].clamp(lo, hi),
            Op::Lerp(a, b, t) => {
                let (x, y) = (cells[a as usize], cells[b as usize]);
                x + t * (y - x)
            }
            Op::Pow(a, b) => cells[a as usize].powf(cells[b as usize]),
            Op::Evaluate(h, ..) => arena.eval(cells[h as usize] as u32, cells, None),
            Op::EvalAt(h, x, ..) => arena.eval(cells[h as usize] as u32, cells, Some(cells[x as usize])),
        }
    }

    /// Given a desired output value, which input to write and what value.
    /// Returns `None` when the request must be dropped: the inverse is
    /// undefined for the current values (division by zero) or the desired
    /// value is outside the operator's domain.
    #[inline]
    pub fn invert(&self, cells: &[f64], desired: f64) -> Option<(CellIdx, f64)> {
        let v = |c: CellIdx| cells[c as usize];
        Some(match *self {
            Op::Add(a, b) => (a, desired - v(b)),
            Op::Sub(a, b) => (a, desired + v(b)),
            Op::Mul(a, b) => {
                let d = v(b);
                if d == 0.0 { return None; }
                (a, desired / d)
            }
            Op::Div(a, b) => {
                let d = v(b);
                if d == 0.0 || d.is_nan() { return None; }
                (a, desired * d)
            }
            // Projections: the input is asked for the projected value.
            Op::Min(a, b) => (a, nan_min(desired, v(b))),
            Op::Max(a, b) => (a, nan_max(desired, v(b))),
            // While `a` has no value the request belongs to the fallback.
            Op::Default(a, b) => {
                if v(a).is_nan() { (b, desired) } else { (a, desired) }
            }
            Op::Negate(a) => (a, -desired),
            Op::Round(a) => {
                if !desired.is_finite() { return None; }
                (a, desired.round())
            }
            Op::Floor(a) => {
                if !desired.is_finite() { return None; }
                (a, desired.floor())
            }
            Op::Scale(a, k) => {
                if k == 0.0 { return None; }
                (a, desired / k)
            }
            Op::Offset(a, k) => (a, desired - k),
            Op::Clamp(a, lo, hi) => (a, desired.clamp(lo, hi)),
            Op::Lerp(a, b, t) => {
                // desired = a(1-t) + t b
                if t == 1.0 { return None; }
                (a, (desired - t * v(b)) / (1.0 - t))
            }
            // Symbolic inverses are out of scope (plan 2, follow-ups).
            Op::Pow(..) | Op::Evaluate(..) | Op::EvalAt(..) => return None,
        })
    }

    /// Input cells, without allocating: `(first, Some(second))` for binary ops.
    #[inline(always)]
    pub fn input_pair(&self) -> (CellIdx, Option<CellIdx>) {
        match *self {
            Op::Add(a, b) | Op::Sub(a, b) | Op::Mul(a, b) | Op::Div(a, b) | Op::Min(a, b) | Op::Max(a, b) | Op::Default(a, b) | Op::Lerp(a, b, _) | Op::Pow(a, b) | Op::EvalAt(a, b, ..) => (a, Some(b)),
            Op::Negate(a) | Op::Round(a) | Op::Floor(a) | Op::Scale(a, _) | Op::Offset(a, _) | Op::Clamp(a, _, _) | Op::Evaluate(a, ..) => (a, None),
        }
    }

    /// Range into `Program::extra` of further inputs (only `Evaluate`/`EvalAt`).
    #[inline(always)]
    pub fn extra_range(&self) -> std::ops::Range<usize> {
        match *self {
            Op::Evaluate(_, start, n) | Op::EvalAt(_, _, start, n) => start as usize..start as usize + n as usize,
            _ => 0..0,
        }
    }

    /// All input cells, direct and extra.
    pub fn inputs<'a>(&self, extra: &'a [CellIdx]) -> impl Iterator<Item = CellIdx> + 'a {
        let (a, b) = self.input_pair();
        std::iter::once(a).chain(b).chain(extra[self.extra_range()].iter().copied())
    }

    pub fn kind_name(&self) -> &'static str {
        match self {
            Op::Add(..) => "add",
            Op::Sub(..) => "sub",
            Op::Mul(..) => "mul",
            Op::Div(..) => "div",
            Op::Min(..) => "min",
            Op::Max(..) => "max",
            Op::Default(..) => "default",
            Op::Negate(..) => "negate",
            Op::Round(..) => "round",
            Op::Floor(..) => "floor",
            Op::Scale(..) => "scale",
            Op::Offset(..) => "offset",
            Op::Clamp(..) => "clamp",
            Op::Lerp(..) => "lerp",
            Op::Pow(..) => "pow",
            Op::Evaluate(..) => "evaluate",
            Op::EvalAt(..) => "evalAt",
        }
    }
}

/// Description of an operator before its inputs are bound to cells.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OpSpec {
    Add,
    Sub,
    Mul,
    Div,
    Min,
    Max,
    Default,
    Negate,
    Round,
    Floor,
    Scale { k: f64 },
    Offset { k: f64 },
    Clamp { lo: f64, hi: f64 },
    Lerp { t: f64 },
    Pow,
    Evaluate,
    EvalAt,
}

impl OpSpec {
    pub fn arity(&self) -> usize {
        match self {
            OpSpec::Add | OpSpec::Sub | OpSpec::Mul | OpSpec::Div | OpSpec::Min | OpSpec::Max | OpSpec::Default | OpSpec::Lerp { .. } | OpSpec::Pow | OpSpec::EvalAt => 2,
            OpSpec::Negate | OpSpec::Round | OpSpec::Floor | OpSpec::Scale { .. } | OpSpec::Offset { .. } | OpSpec::Clamp { .. } | OpSpec::Evaluate => 1,
        }
    }

    /// Bind to input cells. `Evaluate`/`EvalAt` take their cell leaves after
    /// the direct inputs and park them in `extra`.
    pub fn bind(&self, inputs: &[CellIdx], extra: &mut Vec<CellIdx>) -> Op {
        debug_assert!(inputs.len() == self.arity() || matches!(self, OpSpec::Evaluate | OpSpec::EvalAt));
        let park = |extra: &mut Vec<CellIdx>, leaves: &[CellIdx]| {
            let start = extra.len() as u32;
            extra.extend_from_slice(leaves);
            (start, leaves.len() as u8)
        };
        match *self {
            OpSpec::Add => Op::Add(inputs[0], inputs[1]),
            OpSpec::Sub => Op::Sub(inputs[0], inputs[1]),
            OpSpec::Mul => Op::Mul(inputs[0], inputs[1]),
            OpSpec::Div => Op::Div(inputs[0], inputs[1]),
            OpSpec::Min => Op::Min(inputs[0], inputs[1]),
            OpSpec::Max => Op::Max(inputs[0], inputs[1]),
            OpSpec::Default => Op::Default(inputs[0], inputs[1]),
            OpSpec::Negate => Op::Negate(inputs[0]),
            OpSpec::Round => Op::Round(inputs[0]),
            OpSpec::Floor => Op::Floor(inputs[0]),
            OpSpec::Scale { k } => Op::Scale(inputs[0], k),
            OpSpec::Offset { k } => Op::Offset(inputs[0], k),
            OpSpec::Clamp { lo, hi } => Op::Clamp(inputs[0], lo, hi),
            OpSpec::Lerp { t } => Op::Lerp(inputs[0], inputs[1], t),
            OpSpec::Pow => Op::Pow(inputs[0], inputs[1]),
            OpSpec::Evaluate => {
                let (start, n) = park(extra, &inputs[1..]);
                Op::Evaluate(inputs[0], start, n)
            }
            OpSpec::EvalAt => {
                let (start, n) = park(extra, &inputs[2..]);
                Op::EvalAt(inputs[0], inputs[1], start, n)
            }
        }
    }
}
