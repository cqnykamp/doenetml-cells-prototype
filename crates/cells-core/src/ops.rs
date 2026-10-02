//! Operators: the numeric functions that compute derived cells.
//! Literal parameters are stored inline in the instruction, not in cells.

use crate::document::CellIdx;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Op {
    Add(CellIdx, CellIdx),
    Sub(CellIdx, CellIdx),
    Mul(CellIdx, CellIdx),
    Negate(CellIdx),
    Scale(CellIdx, f64),
    Offset(CellIdx, f64),
    Clamp(CellIdx, f64, f64),
    /// `a + t * (b - a)`
    Lerp(CellIdx, CellIdx, f64),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Instr {
    pub out: CellIdx,
    pub op: Op,
}

impl Op {
    #[inline(always)]
    pub fn eval(&self, cells: &[f64]) -> f64 {
        match *self {
            Op::Add(a, b) => cells[a as usize] + cells[b as usize],
            Op::Sub(a, b) => cells[a as usize] - cells[b as usize],
            Op::Mul(a, b) => cells[a as usize] * cells[b as usize],
            Op::Negate(a) => -cells[a as usize],
            Op::Scale(a, k) => cells[a as usize] * k,
            Op::Offset(a, k) => cells[a as usize] + k,
            Op::Clamp(a, lo, hi) => cells[a as usize].clamp(lo, hi),
            Op::Lerp(a, b, t) => {
                let (x, y) = (cells[a as usize], cells[b as usize]);
                x + t * (y - x)
            }
        }
    }

    pub fn inputs(&self) -> Vec<CellIdx> {
        match *self {
            Op::Add(a, b) | Op::Sub(a, b) | Op::Mul(a, b) | Op::Lerp(a, b, _) => vec![a, b],
            Op::Negate(a) | Op::Scale(a, _) | Op::Offset(a, _) | Op::Clamp(a, _, _) => vec![a],
        }
    }

    pub fn kind_name(&self) -> &'static str {
        match self {
            Op::Add(..) => "add",
            Op::Sub(..) => "sub",
            Op::Mul(..) => "mul",
            Op::Negate(..) => "negate",
            Op::Scale(..) => "scale",
            Op::Offset(..) => "offset",
            Op::Clamp(..) => "clamp",
            Op::Lerp(..) => "lerp",
        }
    }
}

/// Description of an operator before its inputs are bound to cells.
#[derive(Debug, Clone, PartialEq)]
pub enum OpSpec {
    Add,
    Sub,
    Mul,
    Negate,
    Scale { k: f64 },
    Offset { k: f64 },
    Clamp { lo: f64, hi: f64 },
    Lerp { t: f64 },
}

impl OpSpec {
    pub fn arity(&self) -> usize {
        match self {
            OpSpec::Add | OpSpec::Sub | OpSpec::Mul | OpSpec::Lerp { .. } => 2,
            OpSpec::Negate | OpSpec::Scale { .. } | OpSpec::Offset { .. } | OpSpec::Clamp { .. } => 1,
        }
    }

    pub fn bind(&self, inputs: &[CellIdx]) -> Op {
        debug_assert_eq!(inputs.len(), self.arity());
        match *self {
            OpSpec::Add => Op::Add(inputs[0], inputs[1]),
            OpSpec::Sub => Op::Sub(inputs[0], inputs[1]),
            OpSpec::Mul => Op::Mul(inputs[0], inputs[1]),
            OpSpec::Negate => Op::Negate(inputs[0]),
            OpSpec::Scale { k } => Op::Scale(inputs[0], k),
            OpSpec::Offset { k } => Op::Offset(inputs[0], k),
            OpSpec::Clamp { lo, hi } => Op::Clamp(inputs[0], lo, hi),
            OpSpec::Lerp { t } => Op::Lerp(inputs[0], inputs[1], t),
        }
    }
}
