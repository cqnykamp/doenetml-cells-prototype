//! Prototype DoenetML core organized around a flat list of `f64` cells.
//!
//! Cells are the nodes of the dependency graph. Components and props are a
//! naming layer over cell indices. See `CONTEXT.md` at the repository root for
//! the vocabulary used throughout this crate.

pub mod build;
pub mod components;
pub mod dast;
pub mod document;
pub mod error;
pub mod eval;
pub mod expr;
pub mod geo;
pub mod invert;
pub mod ops;
pub mod program;
pub mod reference;
pub mod sticky;
pub mod test_utils;

pub use document::{CellIdx, Child, CompIdx, Components, Document, LoadTimings, NONE, Repeat, Request, ScopeId, Structure, TEXT_BIT, Tick};
pub use error::{Error, Result};
pub use eval::{DirtyClosure, Evaluator, FullRecompute};
pub use geo::{Pivot, Produced, RigidOpts, VecOp};
pub use ops::{Instr, Op, OpSpec};
pub use invert::{Inversion, PointRequest};
pub use program::Program;
