//! Prototype DoenetML core organized around a flat list of `f64` cells.
//!
//! Cells are the nodes of the dependency graph. Components and props are a
//! naming layer over cell indices. See `CONTEXT.md` at the repository root for
//! the vocabulary used throughout this crate.
//!
//! The modules, by what they are for:
//!
//! - `dast` — the input: the parser's DAST in flat, columnar form.
//! - `components` — the tag vocabulary: each kind's props and where their
//!   values come from. The build plans from it; the component table names
//!   cells with it.
//! - `build` — DAST to an unscheduled program: compile, expand, resolve,
//!   emit.
//! - `program` — the instruction set the build emits and a tick runs:
//!   operators, their inverses, the scheduled instruction list.
//! - `tick` — run time: snapping, inversion, recompute.
//! - `document` — the loaded document: load and rebuild, and the API a
//!   renderer or test reads.
//! - `testing` — the reference oracle and test helpers.

pub mod build;
pub mod components;
pub mod dast;
mod document;
mod error;
pub mod program;
pub mod testing;
pub mod tick;

// The API of a loaded document: load it, read it, send it requests.
pub use document::{
    CellIdx, Child, CompIdx, ComponentTable, Document, LoadOptions, LoadTimings, NONE, TEXT_BIT,
};
pub use error::{Error, Result};
pub use tick::eval::{DirtyClosure, Evaluator, FullRecompute};
pub use tick::invert::PointRequest;
pub use tick::{Request, Tick};
