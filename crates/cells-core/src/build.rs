//! Build a [`Document`] from a flat DAST: compile the document's templates,
//! expand them into components, resolve references, merge aliased props into
//! shared cells, and emit the instruction list.
//!
//! The build has two halves.
//!
//! **Compile** walks the DAST once and produces one [`Template`] per repeat
//! (plus one for the document itself). A template holds, per element, the
//! kind, the name, and a *plan* for every prop: a parsed literal, a default,
//! an operator over reference plans, or a reference plan. A reference plan
//! is a path resolved against the template nesting: how many template levels
//! up the name was found, which element it is, which `[index]` expressions
//! select iterations, which prop. Nothing in a plan depends on which
//! iteration it will be instantiated in, so all string work (tag and
//! attribute matching, literal parsing, name lookup) happens once per
//! template element rather than once per iteration.
//!
//! **Expand** stamps templates into components. Each instance of a template
//! is a *scope*: scope 0 is the document, every iteration of a repeat is a
//! scope whose parent is the scope the repeat sits in. Scopes carry dense
//! tables (element -> component) so a reference plan resolves with array
//! reads: walk `hops` parents, index the element table, follow indices into
//! iteration scopes. Scope ids are stable across rebuilds of one document
//! (the table only grows), which is how iteration counts and essential
//! values carry over: both are stored per (scope, template slot).
//!
//! **Emit**: union-find over aliases makes cells; cells are numbered
//! (essential, then fixed, then derived); operators are bound; and the
//! instruction list is scheduled, with a fast path when creation order is
//! already a valid evaluation order.
//!
//! **Compile** lives in `compile.rs` (the walk and each element's shape),
//! `attrs.rs` (attribute and math sources), `refs.rs` (names and reference
//! paths), `expr.rs` (the parse arena for math text), `geometry.rs` (the
//! planned kinds), `choice.rs` with `condition.rs`, `copies.rs`
//! (`extend`) and `fix.rs` (`fixed`); what it produces is in `plan.rs`.
//! **Expand** lives in `expand.rs` (with the builder's state),
//! `expand_math.rs` and `resolve.rs`, with `scoring.rs` for credit and
//! section numbers; **Emit** in `emit.rs`. `choice.rs` has a compile half
//! and an expand half. This file holds the entry point, [`build`], what it
//! returns ([`Unscheduled`]) and what one build hands the next ([`Prior`]).

use std::collections::HashMap;

use cells_sym::{SymEngine, Tree};

use crate::components::{ComponentKind, PropFrom, prop};
use crate::dast::{Dast, NodeId, NodeKind, StrId, StringTable};
use crate::document::{CellIdx, CompIdx, ComponentTable, Document, NONE, Repeat, ScopeId, ScopeTable, Structure, TEXT_BIT};
use crate::error::{Error, Result};
use expr::{Arena, Expr, ExprId, Parser, Token};
use crate::program::geo::{Pivot, RigidOpts, VecOp};
use crate::program::ops::{Instr, OpSpec, Post, SymKind};
use crate::program::{Program, Sym};

mod attrs;
mod choice;
mod compile;
mod condition;
mod copies;
mod emit;
mod expand;
mod expand_math;
mod expr;
mod fix;
mod geometry;
mod plan;
mod refs;
mod resolve;
mod scoring;

type SlotId = u32;
type TemplateId = usize;
type ElemId = usize;
type RefId = usize;
type ChoiceId = usize;

use expand::*;
use plan::*;

/// Build with `engine` holding the document's expressions (the same engine
/// across rebuilds, so essential math cells keep valid handles).
pub fn build(dast: &Dast, prior: &Prior, engine: &mut dyn SymEngine) -> Result<Unscheduled> {
    let profile = std::env::var_os("CELLS_BUILD_PROFILE").is_some();
    let clock = web_time::Instant::now();
    let lap = |what: &str| {
        if profile {
            eprintln!("  build/{what}: {:.2?}", clock.elapsed());
        }
    };
    let compiled = Compiler::compile(dast)?;
    lap("compile");
    let mut b = Builder::new(&compiled, prior, engine);
    b.expand_all()?;
    lap("expand");
    b.resolve_all()?;
    lap("resolve");
    let u = b.finish()?;
    lap("cells, program");
    Ok(u)
}

/// One build pass with nothing carried over: every repeat has zero
/// iterations. `Document::load` iterates this to a fixed point.
pub fn build_once(dast: &Dast, engine: &mut dyn SymEngine) -> Result<Unscheduled> {
    build(dast, &Prior::default(), engine)
}

/// A built document whose program has not yet been scheduled.
pub struct Unscheduled {
    cells: Vec<f64>,
    n_essential: usize,
    n_fixed: usize,
    instrs: Vec<Instr>,
    comps: ComponentTable,
    strings: StringTable,
    root: CompIdx,
    structure: Structure,
    extra: Vec<CellIdx>,
    math: Vec<bool>,
    tapes: Vec<cells_sym::tape::Tape>,
    /// Human-readable owner of a cell, e.g. "p1.x". Computed lazily because
    /// a cycle error is the only consumer.
    cell_label: Box<dyn Fn(CellIdx) -> String>,
}

impl Unscheduled {
    /// Schedule the program, moving the symbolic engine the build used into
    /// it. On error the engine stays with the caller.
    pub fn schedule(self, dast: std::sync::Arc<Dast>, engine: &mut Box<dyn SymEngine>) -> Result<Document> {
        let n = self.cells.len();
        let sym = Sym::new(std::mem::replace(engine, Box::new(cells_sym::flat::Flat::new())));
        let program = match Program::schedule(self.instrs, n, sym, self.extra, self.math) {
            Ok(mut p) => {
                p.tapes = self.tapes;
                p
            }
            Err((cell, sym)) => {
                *engine = sym.into_engine();
                return Err(Error::Cycle((self.cell_label)(cell)));
            }
        };
        Ok(Document::new(self.cells, self.n_essential, self.n_fixed, program, self.comps, self.strings, self.root, self.structure, dast))
    }
}

// ---------------------------------------------------------------------------
// Prior: what earlier builds of the same document contribute
// ---------------------------------------------------------------------------

/// Carried from one build of a document to the next: the stable scope table,
/// each repeat instance's iteration count, and every essential value ever
/// held, stored per (scope, template slot) so an iteration that disappears
/// and reappears comes back as it was left.
#[derive(Debug, Clone, Default)]
pub struct Prior {
    /// The last build's structure: its scope table, seed and options, and in
    /// `values` every essential value it held.
    structure: Structure,
    counts: HashMap<(ScopeId, NodeId), u32>,
}

impl Prior {
    /// The prior of a first build.
    pub fn new(seed: u64, sample_with_engine: bool) -> Prior {
        Prior { structure: Structure { seed, sample_with_engine, ..Structure::default() }, counts: HashMap::new() }
    }

    /// Build a prior from a document, moving its structure out (the
    /// document is about to be replaced). `restore` puts it back on error.
    pub fn take_from(doc: &mut Document) -> Prior {
        let counts = doc.structure.repeats.iter().map(|r| ((r.scope, r.node), doc.repeat_count(r))).collect();
        let mut structure = std::mem::take(&mut doc.structure);
        structure.values.resize(structure.scopes.len(), Vec::new());
        for (&(scope, slot), &v) in structure.essential_slots.iter().zip(&doc.cells[..doc.n_essential]) {
            let row = &mut structure.values[scope as usize];
            if row.len() <= slot as usize {
                row.resize(slot as usize + 1, None);
            }
            row[slot as usize] = Some(v);
        }
        Prior { structure, counts }
    }

    pub fn restore(self, doc: &mut Document) {
        doc.structure = self.structure;
    }
}
