//! The loaded document: the cell array, the program that derives cells, and
//! the columnar component layer that names cells for references and the
//! renderer.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use cells_sym::SymEngine;

use crate::components::ComponentKind;
use crate::dast::{Dast, NodeId, StrId, StringTable};
use crate::invert::PointRequest;
use crate::program::Program;

mod sticky;
mod table;

pub type CellIdx = u32;
pub type CompIdx = u32;
/// A name scope: 0 is the document, every other scope is one iteration of a
/// repeat (see `build.rs`).
pub type ScopeId = u32;

/// One expanded `<repeatForSequence>`.
#[derive(Debug, Clone)]
pub struct Repeat {
    pub comp: CompIdx,
    pub node: NodeId,
    /// Scope the repeat element sits in.
    pub scope: ScopeId,
    /// One scope per iteration, in order.
    pub iter_scopes: Vec<ScopeId>,
    /// Iterations the build used; a differing `count` cell triggers a rebuild.
    pub n: u32,
}

/// What a build knew about the document's shape, carried into the next
/// build so iteration counts and essential values survive.
#[derive(Debug, Clone, Default)]
pub struct Structure {
    /// (parent scope, repeat element, 1-based position) per scope; entry 0
    /// is the document. Ids are stable across rebuilds: the table only grows.
    pub scopes: Vec<(ScopeId, NodeId, u32)>,
    pub scope_index: HashMap<(ScopeId, NodeId, u32), ScopeId>,
    /// Per essential cell, in cell order: its essential key as (scope,
    /// template slot). The template slot identifies the element and prop
    /// within the scope's template. See `CONTEXT.md`, essential key.
    pub essential_slots: Vec<(ScopeId, u32)>,
    /// `values[scope][template slot]`: the last value of every essential
    /// cell that has ever existed, so an iteration that disappears and
    /// reappears returns as it was left. Rows fill lazily.
    pub values: Vec<Vec<Option<f64>>>,
    pub repeats: Vec<Repeat>,
    pub counts_used: Vec<u32>,
    /// Per repeat (same order as `repeats`): how many repeats must be
    /// expanded in sequence before this one's count is known, plus one.
    /// Nesting adds one; a count that reads a cell inside another repeat's
    /// iterations adds one. Load takes `structural_depth + 1` passes.
    pub repeat_depths: Vec<u32>,
    /// Per repeat: whether its count reads a cell inside another repeat's
    /// iterations (the avoidable kind of depth, reported by `warnings`).
    pub repeat_cross_reads: Vec<bool>,
    /// The largest `repeat_depths` entry; 0 without repeats.
    pub structural_depth: u32,
    /// The document seed load-time choices draw from (plan 6).
    pub seed: u64,
}

/// Upper bound on build passes before the structure must have settled.
const MAX_PASSES: usize = 8;

pub const NONE: u32 = u32::MAX;
/// Set on a `child_list` entry whose low bits are a string id, not a component.
pub const TEXT_BIT: u32 = 1 << 31;

/// Components as parallel arrays indexed by `CompIdx`. Props are implicit:
/// component `c` of kind `k` owns `prop_cells[prop_base[c] + i]` for each
/// `i` in `k.prop_defs()`.
#[derive(Debug, Clone, Default)]
pub struct Components {
    pub kind: Vec<ComponentKind>,
    /// String id of the name, or `NONE`.
    pub name: Vec<StrId>,
    pub parent: Vec<CompIdx>,
    pub prop_base: Vec<u32>,
    pub prop_cells: Vec<CellIdx>,
    pub child_start: Vec<u32>,
    pub child_count: Vec<u32>,
    /// Component indices, or `TEXT_BIT | string id` for text children.
    pub child_list: Vec<u32>,
    /// DAST element each component came from (NONE if synthesized) and the
    /// scope it was created in. Together they identify a component across
    /// rebuilds, which lets a renderer keep its tree keyed by identity.
    pub node: Vec<u32>,
    pub scope: Vec<ScopeId>,
}

impl Components {
    pub fn len(&self) -> usize {
        self.kind.len()
    }

    pub fn is_empty(&self) -> bool {
        self.kind.is_empty()
    }

    /// A sticky group's members: its children that take part, with repeats
    /// and collects replaced by what they expanded to, as the current core's
    /// composites are.
    pub fn sticky_members(&self, group: CompIdx) -> Vec<CompIdx> {
        let mut out = Vec::new();
        let mut stack = vec![group];
        while let Some(c) = stack.pop() {
            let start = self.child_start[c as usize] as usize;
            let kids = &self.child_list[start..start + self.child_count[c as usize] as usize];
            for &k in kids.iter().rev() {
                if k & TEXT_BIT != 0 {
                    continue;
                }
                match self.kind[k as usize] {
                    ComponentKind::RepeatForSequence | ComponentKind::Collect => stack.push(k),
                    kind if kind.sticky_layout().is_some() => out.push(k),
                    _ => {}
                }
            }
        }
        out
    }

    pub fn heap_bytes(&self) -> usize {
        self.kind.capacity() * std::mem::size_of::<ComponentKind>()
            + 4 * (self.name.capacity()
                + self.parent.capacity()
                + self.prop_base.capacity()
                + self.prop_cells.capacity()
                + self.child_start.capacity()
                + self.child_count.capacity()
                + self.child_list.capacity()
                + self.node.capacity()
                + self.scope.capacity())
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Child<'a> {
    Component(CompIdx),
    Text(&'a str),
}

#[derive(Debug, Clone)]
pub struct Document {
    /// All cells. Essential cells come first, then fixed, then derived.
    pub cells: Vec<f64>,
    /// Number of essential cells; `cells[..n_essential]` are essential.
    pub n_essential: usize,
    /// Fixed cells follow the essential ones: constants that are not state
    /// (iteration indices, collect counts, the missing-referent NaN).
    pub n_fixed: usize,
    pub program: Program,
    pub comps: Components,
    /// Names and text, shared with the DAST they came from.
    pub strings: StringTable,
    /// Index of the root `<document>` component.
    pub root: CompIdx,
    pub structure: Structure,
    /// The document as loaded, kept for rebuilds.
    pub dast: Arc<Dast>,
    /// Sticky groups as cells, for the request pre-pass (plan 4).
    sticky: Vec<sticky::StickyTable>,
}

/// A renderer's ask to change one cell to a value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Request {
    pub cell: CellIdx,
    pub value: f64,
}

/// What one tick changed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tick {
    /// Cells whose value changed, essential ones first in request order,
    /// then derived ones in schedule order. No duplicates within each part.
    pub changed: Vec<CellIdx>,
    /// Requests that were dropped because an inverse was undefined or the
    /// request landed on a fixed cell.
    pub dropped: Vec<Request>,
    /// The tick changed a structural cell and the document was rebuilt:
    /// cell indices and the component table are new, `changed` is empty.
    pub rebuilt: bool,
    /// The rebuild failed and the document is unchanged from before it.
    pub rebuild_error: Option<String>,
    /// For each changed math cell, its expression as LaTeX, so a renderer
    /// never reads the engine (Plan 5).
    pub latex: Vec<(CellIdx, String)>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct LoadTimings {
    pub deserialize: Duration,
    pub build: Duration,
    pub schedule: Duration,
    pub initial_compute: Duration,
    /// Build passes until repeat counts settled (1 without repeats).
    pub passes: u32,
    /// `Structure::structural_depth` of the settled document.
    pub structural_depth: u32,
}

impl Document {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(cells: Vec<f64>, n_essential: usize, n_fixed: usize, program: Program, comps: Components, strings: StringTable, root: CompIdx, structure: Structure, dast: Arc<Dast>) -> Self {
        let mut doc = Document {
            cells,
            n_essential,
            n_fixed,
            program,
            comps,
            strings,
            root,
            structure,
            dast,
            sticky: Vec::new(),
        };
        doc.sticky = doc.sticky_tables();
        doc
    }

    /// Load from DAST JSON and compute initial values.
    pub fn from_dast_json(json: &str) -> crate::Result<Document> {
        Ok(Self::load_timed(json.as_bytes())?.0)
    }

    /// Load from either wire format (JSON or binary), detected by content.
    pub fn from_bytes(bytes: &[u8]) -> crate::Result<Document> {
        Ok(Self::load_timed(bytes)?.0)
    }

    /// Load from either wire format, timing each stage separately. Build,
    /// schedule and compute repeat until every repeat's iteration count
    /// matches its `count` cell; the timings sum over passes.
    pub fn load_timed(bytes: &[u8]) -> crate::Result<(Document, LoadTimings)> {
        Self::load_timed_with(bytes, Box::new(cells_sym::flat::Flat::new()))
    }

    /// `load_timed` with a chosen symbolic engine (engine A by default).
    pub fn load_timed_with(bytes: &[u8], engine: Box<dyn SymEngine>) -> crate::Result<(Document, LoadTimings)> {
        Self::load_timed_seeded(bytes, engine, 0)
    }

    /// `load_timed_with` and the document seed that load-time choices draw
    /// from (plan 6): one seed gives one variant of the document.
    pub fn load_timed_seeded(bytes: &[u8], engine: Box<dyn SymEngine>, seed: u64) -> crate::Result<(Document, LoadTimings)> {
        let mut t = LoadTimings::default();
        let clock = web_time::Instant::now();
        let dast = Arc::new(crate::dast::load(bytes)?);
        t.deserialize = clock.elapsed();
        let doc = Self::build_settled(dast, &mut t, engine, seed)?;
        Ok((doc, t))
    }

    /// Load with a document seed.
    pub fn from_bytes_seeded(bytes: &[u8], seed: u64) -> crate::Result<Document> {
        Ok(Self::load_timed_seeded(bytes, Box::new(cells_sym::flat::Flat::new()), seed)?.0)
    }

    /// Load with a chosen symbolic engine.
    pub fn from_bytes_with(bytes: &[u8], engine: Box<dyn SymEngine>) -> crate::Result<Document> {
        Ok(Self::load_timed_with(bytes, engine)?.0)
    }

    pub fn from_dast(dast: Arc<Dast>) -> crate::Result<Document> {
        Self::build_settled(dast, &mut LoadTimings::default(), Box::new(cells_sym::flat::Flat::new()), 0)
    }

    fn build_settled(dast: Arc<Dast>, t: &mut LoadTimings, mut engine: Box<dyn SymEngine>, seed: u64) -> crate::Result<Document> {
        let mut prior = crate::build::Prior::default();
        prior.seed = seed;
        for _ in 0..MAX_PASSES {
            let clock = web_time::Instant::now();
            let unscheduled = crate::build::build(&dast, &prior, &mut *engine)?;
            t.build += clock.elapsed();

            let clock = web_time::Instant::now();
            let mut doc = unscheduled.schedule(dast.clone(), &mut engine)?;
            t.schedule += clock.elapsed();

            let clock = web_time::Instant::now();
            doc.recompute();
            t.initial_compute += clock.elapsed();
            t.passes += 1;
            if doc.structure_settled() {
                t.structural_depth = doc.structure.structural_depth;
                return Ok(doc);
            }
            prior = crate::build::Prior::take_from(&mut doc);
            engine = doc.take_engine();
        }
        Err(crate::Error::UnstableStructure(MAX_PASSES))
    }

    /// Move the symbolic engine out, leaving an empty one (the document is
    /// about to be replaced by a rebuild).
    fn take_engine(&mut self) -> Box<dyn SymEngine> {
        std::mem::take(&mut self.program.sym).into_engine()
    }

    fn put_engine(&mut self, engine: Box<dyn SymEngine>) {
        self.program.sym = crate::program::Sym::new(engine);
        // The memo was dropped with the old `Sym`: recompute so it refills.
        self.program.run_all(&mut self.cells);
    }

    /// Name of the symbolic engine ("A" or "R").
    pub fn engine_name(&self) -> &'static str {
        self.program.sym.engine.borrow().name()
    }

    /// Parse math text (what a student typed) into the engine: the value to
    /// request on a mathInput's `expr` cell.
    pub fn parse_math(&self, text: &str) -> std::result::Result<f64, String> {
        Ok(self.program.sym.engine.borrow_mut().parse(text)? as f64)
    }

    /// The expression a math cell holds, as text; empty when blank.
    pub fn math_text(&self, cell: CellIdx) -> String {
        let h = self.cells[cell as usize];
        if h.is_nan() { String::new() } else { self.program.sym.engine.borrow().text(h as cells_sym::Handle) }
    }

    /// The expression a math cell holds, as LaTeX; empty when blank.
    pub fn math_latex(&self, cell: CellIdx) -> String {
        let h = self.cells[cell as usize];
        if h.is_nan() { String::new() } else { self.program.sym.engine.borrow().latex(h as cells_sym::Handle) }
    }

    /// Submit an answer: an ordinary request copying the live response
    /// handle into its `submitted` cell.
    pub fn submit(&mut self, answer: CompIdx) -> Tick {
        let cells = self.comp_cells(answer);
        let (response, submitted) = (cells[0], cells[2]);
        let value = self.cells[response as usize];
        self.request(&[Request { cell: submitted, value }])
    }

    /// Authoring warnings about the loaded document. Today: repeats whose
    /// count reads a cell inside another repeat's iterations, since each
    /// such link costs a full extra build pass and, unlike nesting, is
    /// avoidable (see `Structure::repeat_depths`).
    pub fn warnings(&self) -> Vec<String> {
        let st = &self.structure;
        let mut out = Vec::new();
        for ((r, &d), &cross) in st.repeats.iter().zip(&st.repeat_depths).zip(&st.repeat_cross_reads) {
            if cross {
                let name = self.name(r.comp).map(str::to_string).unwrap_or_else(|| format!("<repeatForSequence>#{}", r.comp));
                out.push(format!("repeat '{name}' has structural depth {d}: its count reads a cell inside another repeat's iterations, so a change there costs {d} build passes instead of one"));
            }
        }
        out
    }

    /// Whether every repeat was expanded with the iteration count its
    /// `count` cell now holds.
    pub fn structure_settled(&self) -> bool {
        self.structure.repeats.iter().all(|r| self.repeat_count(r) == r.n)
    }

    /// The iteration count a repeat's `count` cell currently asks for.
    pub fn repeat_count(&self, r: &Repeat) -> u32 {
        let pi = ComponentKind::RepeatForSequence.prop_index("count").unwrap();
        let v = self.cells[self.comp_cells(r.comp)[pi] as usize];
        if v.is_nan() || v < 0.0 { 0 } else { v.min(u32::MAX as f64) as u32 }
    }

    /// Rebuild from the retained DAST, carrying iteration counts and
    /// essential values over. On error the document is left unchanged.
    pub fn rebuild(&mut self) -> crate::Result<()> {
        let profile = std::env::var_os("CELLS_BUILD_PROFILE").is_some();
        let dast = self.dast.clone();
        let clock = web_time::Instant::now();
        // The value store and the engine move into the new build; on
        // failure they move back.
        let mut prior = crate::build::Prior::take_from(self);
        let mut engine = self.take_engine();
        if profile {
            eprintln!("rebuild/prior: {:.2?}", clock.elapsed());
        }
        let result = (|| {
            for _ in 0..MAX_PASSES {
                let clock = web_time::Instant::now();
                let u = crate::build::build(&dast, &prior, &mut *engine)?;
                if profile {
                    eprintln!("rebuild/build: {:.2?}", clock.elapsed());
                }
                let clock = web_time::Instant::now();
                let mut doc = u.schedule(dast.clone(), &mut engine)?;
                if profile {
                    eprintln!("rebuild/schedule: {:.2?} (creation order valid: {})", clock.elapsed(), doc.program.in_creation_order);
                }
                let clock = web_time::Instant::now();
                doc.recompute();
                if profile {
                    eprintln!("rebuild/recompute: {:.2?}", clock.elapsed());
                }
                if doc.structure_settled() {
                    return Ok(doc);
                }
                prior = crate::build::Prior::take_from(&mut doc);
                engine = doc.take_engine();
            }
            Err(crate::Error::UnstableStructure(MAX_PASSES))
        })();
        match result {
            Ok(doc) => {
                *self = doc;
                Ok(())
            }
            Err(e) => {
                prior.restore(self);
                self.put_engine(engine);
                Err(e)
            }
        }
    }

    /// A `<mathInput>` holds any math value, infinity included; every other
    /// request site rejects an infinite ask as the current core does.
    fn accepts_infinity(&self, cell: CellIdx) -> bool {
        (0..self.comps.len() as CompIdx).any(|c| self.kind(c) == ComponentKind::MathInput && self.comp_cells(c)[0] == cell)
    }

    pub fn is_essential(&self, cell: CellIdx) -> bool {
        (cell as usize) < self.n_essential
    }

    /// A constant that is not state (an iteration index, a math handle, a
    /// `fixed` value): fixed cells follow the essential ones.
    pub fn is_fixed(&self, cell: CellIdx) -> bool {
        (self.n_essential..self.n_essential + self.n_fixed).contains(&(cell as usize))
    }

    pub fn essential_cells(&self) -> &[f64] {
        &self.cells[..self.n_essential]
    }

    /// Recompute all derived cells from the essential cells.
    pub fn recompute(&mut self) {
        self.program.run_all(&mut self.cells);
    }

    /// Write an essential cell and recompute. Panics if `cell` is derived.
    pub fn set_essential(&mut self, cell: CellIdx, value: f64) {
        assert!(self.is_essential(cell), "cell {cell} is derived");
        self.cells[cell as usize] = value;
        self.recompute();
    }

    /// Apply requests: invert each to an essential cell (later requests win
    /// when two land on one cell), recompute, and report what changed.
    pub fn request(&mut self, requests: &[Request]) -> Tick {
        self.request_with(&mut crate::eval::FullRecompute, requests)
    }

    /// Apply a point group: points dragged together, which keep their shape
    /// when one of them is constrained (ADR 0006).
    pub fn request_points(&mut self, points: &[PointRequest]) -> Tick {
        self.request_with_groups(&mut crate::eval::FullRecompute, &[], &[points.to_vec()])
    }

    /// `request` with an explicit recompute strategy.
    pub fn request_with(&mut self, evaluator: &mut (impl crate::eval::Evaluator + ?Sized), requests: &[Request]) -> Tick {
        self.request_with_groups(evaluator, requests, &[])
    }

    /// Scalar requests and point groups in one tick.
    pub fn request_with_groups(&mut self, evaluator: &mut (impl crate::eval::Evaluator + ?Sized), requests: &[Request], groups: &[Vec<PointRequest>]) -> Tick {
        let mut tick = Tick::default();
        // An infinite ask is never meaningful state (NaN is: an emptied
        // input), and the current core rejects it; drop it before inverting.
        let (mut finite, infinite): (Vec<Request>, Vec<Request>) = requests.iter().partition(|r| !r.value.is_infinite() || self.accepts_infinity(r.cell));
        tick.dropped.extend(infinite);
        let mut finite_groups: Vec<Vec<PointRequest>> = Vec::with_capacity(groups.len());
        for g in groups {
            if g.iter().any(|p| p.values.iter().any(|v| v.is_infinite())) {
                tick.dropped.extend(g.iter().map(|p| Request { cell: p.cells[0], value: p.values[0] }));
            } else {
                finite_groups.push(g.clone());
            }
        }
        self.snap_sticky(&mut finite, &mut finite_groups);
        let inv = self.program.invert_requests(&self.cells, self.n_essential, &finite, &finite_groups);
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
            for &c in &tick.changed {
                if self.program.math[c as usize] {
                    tick.latex.push((c, self.math_latex(c)));
                }
            }
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

    /// Resolve a dotted path as the current core's tests write it
    /// (`"g.Ps[2]"`, `"circle1"`, `"r[3].p"`): a name is visible from the
    /// scope of its nearest named ancestor outward; `[k]` picks the k-th
    /// iteration of a repeat or the k-th child of a collect or point list.
    pub fn resolve_path(&self, path: &str) -> Option<CompIdx> {
        let mut cur: Option<CompIdx> = None;
        // After `r[3]` on a repeat: the iteration a following name picks from.
        let mut iteration: Option<ScopeId> = None;
        for part in path.split('.') {
            let (name, indices) = match part.find('[') {
                Some(i) => (&part[..i], &part[i..]),
                None => (part, ""),
            };
            if !name.is_empty() {
                cur = Some(match (cur, iteration.take()) {
                    (Some(repeat), Some(scope)) => self
                        .children(repeat)
                        .filter_map(|ch| match ch {
                            Child::Component(c) if self.comps.scope[c as usize] == scope && self.name(c) == Some(name) => Some(c),
                            _ => None,
                        })
                        .next()?,
                    (scope, None) => self.find_in_scope(scope, name)?,
                    (None, Some(_)) => unreachable!("an iteration always follows a repeat"),
                });
            }
            for idx in indices.trim_end_matches(']').split(']').filter(|s| !s.is_empty()) {
                let k: usize = idx.trim_start_matches('[').parse().ok()?;
                let c = cur?;
                // `s[1][2]`: the second component of the first pick.
                if let Some(scope) = iteration.take() {
                    cur = Some(self.iteration(c, scope).into_iter().nth(k.checked_sub(1)?)?);
                    continue;
                }
                match self.kind(c) {
                    ComponentKind::RepeatForSequence => {
                        let r = self.structure.repeats.iter().find(|r| r.comp == c)?;
                        iteration = Some(*r.iter_scopes.get(k.checked_sub(1)?)?);
                    }
                    ComponentKind::Select => {
                        let mut picks: Vec<ScopeId> = Vec::new();
                        for ch in self.children(c) {
                            if let Child::Component(x) = ch
                                && !picks.contains(&self.comps.scope[x as usize])
                            {
                                picks.push(self.comps.scope[x as usize]);
                            }
                        }
                        iteration = Some(*picks.get(k.checked_sub(1)?)?);
                    }
                    _ => {
                        cur = self
                            .children(c)
                            .filter_map(|ch| match ch {
                                Child::Component(cc) => Some(cc),
                                _ => None,
                            })
                            .nth(k.checked_sub(1)?);
                    }
                }
            }
        }
        // A path ending at `r[3]` names the iteration's single component.
        match (cur, iteration) {
            (Some(repeat), Some(scope)) => match self.iteration(repeat, scope).as_slice() {
                [c] => Some(*c),
                _ => None,
            },
            _ => cur,
        }
    }

    /// The components one iteration of a repeat (or one pick of a select)
    /// contributes.
    fn iteration(&self, repeat: CompIdx, scope: ScopeId) -> Vec<CompIdx> {
        self.children(repeat)
            .filter_map(|ch| match ch {
                Child::Component(c) if self.comps.scope[c as usize] == scope => Some(c),
                _ => None,
            })
            .collect()
    }

    /// The unique component named `name` visible from `scope` (None: the
    /// document): a descendant not hidden inside a repeat, or `scope`
    /// itself. Several visible matches are ambiguous and resolve to nothing,
    /// except that repeat iterations fall back to the first in document
    /// order so plain names inside a repeat keep working for tests.
    fn find_in_scope(&self, scope: Option<CompIdx>, name: &str) -> Option<CompIdx> {
        if let Some(sc) = scope
            && self.name(sc) == Some(name)
        {
            return Some(sc);
        }
        let matches: Vec<CompIdx> = (0..self.comps.len() as CompIdx).filter(|&c| self.name(c) == Some(name) && self.visible_from(scope, c)).collect();
        match matches.as_slice() {
            [c] => Some(*c),
            [] => (0..self.comps.len() as CompIdx).find(|&c| self.name(c) == Some(name) && self.is_descendant(scope, c)),
            many => {
                // Children of a container copy are reached through the copy's
                // name; among bare matches only originals count.
                let originals: Vec<CompIdx> = many.iter().copied().filter(|&c| !self.inside_copy(scope, c)).collect();
                if originals.len() == 1 { Some(originals[0]) } else { None }
            }
        }
    }

    /// Whether `c` is below `scope` with no repeat strictly between them.
    fn visible_from(&self, scope: Option<CompIdx>, c: CompIdx) -> bool {
        let mut p = self.parent(c);
        while let Some(pc) = p {
            if Some(pc) == scope {
                return true;
            }
            if self.kind(pc) == ComponentKind::RepeatForSequence {
                return false;
            }
            // A built case that is not the active one is not there.
            if self.kind(pc) == ComponentKind::Case && self.cells[self.comp_cells(pc)[0] as usize] != 1.0 {
                return false;
            }
            p = self.parent(pc);
        }
        scope.is_none()
    }

    /// Whether a synthesized (copied) component lies between `scope` and `c`.
    fn inside_copy(&self, scope: Option<CompIdx>, c: CompIdx) -> bool {
        let mut cur = Some(c);
        while let Some(x) = cur {
            if Some(x) == scope {
                return false;
            }
            if self.comps.node[x as usize] == NONE && self.kind(x) != ComponentKind::Document {
                return true;
            }
            cur = self.parent(x);
        }
        false
    }

    fn is_descendant(&self, scope: Option<CompIdx>, c: CompIdx) -> bool {
        let mut p = self.parent(c);
        while let Some(pc) = p {
            if Some(pc) == scope {
                return true;
            }
            p = self.parent(pc);
        }
        scope.is_none()
    }
}

/// Heap footprint of a loaded document, by part.
#[derive(Debug, Clone, Copy, Default)]
pub struct MemoryEstimate {
    pub cells: usize,
    pub program: usize,
    pub components: usize,
    pub strings: usize,
    /// Scope table and essential keys kept for rebuilds.
    pub structure: usize,
    /// The retained DAST, kept for rebuilds (its string table is shared
    /// with `strings` and counted there only once by `total`).
    pub dast: usize,
}

impl MemoryEstimate {
    pub fn total(&self) -> usize {
        self.cells + self.program + self.components + self.strings + self.structure + self.dast
    }
}
