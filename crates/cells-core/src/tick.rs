//! A tick at run time: requests are snapped (`snap.rs`, for sticky groups),
//! inverted down to essential cells (`invert.rs`), and the derived cells
//! downstream recomputed (`eval.rs`). The build is not involved; a tick only
//! reads the scheduled [`Program`](crate::Program) and writes cells.

pub mod eval;
pub mod invert;
pub mod snap;
