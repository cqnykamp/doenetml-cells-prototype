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
//! The phases live in `plan.rs` (with `geometry.rs` for the planned kinds
//! and `copies.rs` for `extend`), `expand.rs` and `emit.rs`; this file holds
//! the types they share.

use std::collections::HashMap;

use cells_sym::{SymEngine, Tree};

use crate::components::{ComponentKind, PropFrom};
use crate::dast::{Dast, NodeId, NodeKind, StrId, StringTable};
use crate::document::{CellIdx, CompIdx, Components, Document, NONE, Repeat, ScopeId, Structure, TEXT_BIT};
use crate::error::{Error, Result};
use crate::expr::{Arena, Expr, ExprId, Parser, Token};
use crate::geo::{Pivot, RigidOpts, VecOp};
use crate::ops::{Instr, OpSpec, Post, SymKind};
use crate::program::{Program, Sym};

mod choice;
mod copies;
mod emit;
mod expand;
mod geometry;
mod plan;
mod scoring;

type SlotId = u32;
type TemplateId = usize;
type ElemId = usize;
type PlanId = usize;
type ChoiceId = usize;

// ---------------------------------------------------------------------------
// Compiled templates
// ---------------------------------------------------------------------------

/// A cell an operator reads or a slot aliases, named before any iteration
/// exists: a reference (with a selector into what it names), one of the
/// element's own slots, or a slot of another element in the same template.
#[derive(Debug, Clone, Copy)]
enum Arg {
    Ref(PlanId, Sel),
    Own(u8),
    /// A copy with overridden attributes shares the rest of the original's
    /// essential state through this.
    Elem(ElemId, u8),
}

/// Which cell of a reference's target: the single cell, coordinate `j` of a
/// point-valued prop, or coordinate `j` of item `i` of an array prop.
#[derive(Debug, Clone, Copy)]
enum Sel {
    Whole,
    Coord(u8),
    Item(u8, u8),
}

/// Where a prop's value comes from, before any iteration exists.
#[derive(Debug, Clone)]
enum SourcePlan {
    /// An essential cell with this initial value.
    Literal(f64),
    /// An essential cell with the kind's default value.
    Default(f64),
    /// The matching slot of the `extend` referent.
    Inherit,
    /// Slot `k` of the `extend` referent: an `Inherit` a gate moved.
    InheritFrom(u8),
    /// A constant that is not state.
    Fixed(f64),
    Alias(Arg),
    /// Operator over cells.
    Op(OpSpec, Vec<Arg>),
    /// The 1-based position of the enclosing iteration.
    IterIndex,
    /// Math text: lowered to operators if numeric, else NaN.
    Math(ExprId),
    /// A `<math>`'s `expr` prop: NaN when the math is numeric (it lowers),
    /// else as `SymExpr`.
    MathHandle(ExprId, Post),
    /// A `<math>`'s `value` prop: the literal if the expression is a
    /// number, the lowered operators if numeric, else an `Evaluate` of its
    /// `expr`.
    MathValue(ExprId),
    /// A math cell from math text (`Compiled::sym_text`): a fixed handle
    /// without cell leaves, an alias of a lone math leaf, else an
    /// `Instantiate` instruction over the leaves.
    SymExpr(ExprId, Post),
    /// An essential math cell (an unbound mathInput's `expr`, an answer's
    /// `submitted`) holding this expression, or blank (NaN).
    MathEssential(Option<Tree>),
    /// Head of a vector instruction; this slot is output 0, the next
    /// `n_out - 1` slots are `VecOut`.
    Vec(VecOp, Vec<Arg>),
    /// Output `k` of the vector instruction headed at own slot `head`.
    VecOut(u8, u8),
}

impl SourcePlan {
    fn reference(p: PlanId) -> Self {
        SourcePlan::Alias(Arg::Ref(p, Sel::Whole))
    }
    fn coord(p: PlanId, j: usize) -> Self {
        SourcePlan::Alias(Arg::Ref(p, Sel::Coord(j as u8)))
    }
    fn item(p: PlanId, i: usize, j: usize) -> Self {
        SourcePlan::Alias(Arg::Ref(p, Sel::Item(i as u8, j as u8)))
    }
    fn own(slot: u8) -> Self {
        SourcePlan::Alias(Arg::Own(slot))
    }
    /// Operator over the element's own slots.
    fn computed(op: OpSpec, args: Vec<u8>) -> Self {
        SourcePlan::Op(op, args.into_iter().map(Arg::Own).collect())
    }
    fn vector(op: VecOp, args: Vec<u8>) -> Self {
        SourcePlan::Vec(op, args.into_iter().map(Arg::Own).collect())
    }
}

#[derive(Debug, Clone)]
enum Child {
    Elem(ElemId),
    Text(StrId),
    /// A `$ref` child: a copy or a number, decided when it resolves. The
    /// flag says whether the path carries an index.
    Macro(PlanId, bool),
}

#[derive(Debug, Clone)]
enum Body {
    Plain,
    Repeat {
        template: TemplateId,
    },
    Collect {
        from: PlanId,
        kind: ComponentKind,
    },
    /// `<pointList extend="$l.points">`: children are synthesized points.
    PointList {
        from: PlanId,
    },
    /// A `<conditionalContent>` or `<select>` (plan 6): `Compiled::choices`.
    Choice(ChoiceId),
}

/// A choice as compiled (plan 6, ADR 0009): one template per branch, its
/// branch interface, and how it chooses.
#[derive(Debug, Clone)]
struct ChoiceDef {
    /// The choice element: (template, element).
    at: (TemplateId, ElemId),
    /// `<conditionalContent>` (reactive) or `<select>` (load-time).
    reactive: bool,
    /// One template per case or option, in document order.
    branches: Vec<TemplateId>,
    /// Reactive: each case's `condition` attribute (None: an else).
    conditions: Vec<Option<u32>>,
    /// Load-time: picks, with replacement or not, and a weight per option.
    num_to_select: u32,
    with_replacement: bool,
    weights: Vec<f64>,
    /// The branch interface (`CONTEXT.md`): name -> kind and the element
    /// carrying it in each branch. Filled once every element is planned.
    iface: HashMap<String, (ComponentKind, Vec<ElemId>)>,
    /// Interface names that references use, in first-use order; a
    /// `Step::Iface` holds an index here.
    used: Vec<String>,
}

/// The template itself, as the parent of its top-level elements.
const ROOT_SCOPE: ElemId = usize::MAX;

#[derive(Debug, Clone)]
struct Elem {
    node: NodeId,
    kind: ComponentKind,
    name: StrId,
    /// Parent element within the template (`ROOT_SCOPE` at the top). A
    /// name is visible from every ancestor, as in the current core's
    /// resolver; repeats hide their children behind their own name because
    /// each repeat body is its own template.
    name_scope: ElemId,
    /// First of this element's prop slots within the template's slot space.
    /// Assigned once every element is planned, since planned kinds add
    /// hidden slots after the public props.
    slot_off: u32,
    props: Vec<SourcePlan>,
    children: Vec<Child>,
    extend: Option<PlanId>,
    /// A child of a container copy (`<graph extend="$g"/>`): every prop
    /// aliases the original's, and the children are clones too.
    cloned: bool,
    /// Named essential slots of a planned kind (a line's default points, a
    /// circle's essential radius), so a copy with overridden attributes can
    /// share the ones it does not override.
    roles: HashMap<&'static str, u8>,
    body: Body,
}

#[derive(Debug, Clone, Default)]
struct Template {
    /// (template, element) of the repeat this template belongs to.
    parent: Option<(TemplateId, ElemId)>,
    elems: Vec<Elem>,
    n_slots: u32,
    /// Children of the template itself (the document, or the repeat body).
    children: Vec<Child>,
    /// Compile-time name table: for every ancestor (and `ROOT_SCOPE`), the
    /// descendants carrying each name. More than one is an ambiguity.
    names: HashMap<(ElemId, String), Vec<ElemId>>,
}

/// One step of a resolved reference path.
#[derive(Debug, Clone)]
enum Step {
    /// An element of the current template.
    Elem(ElemId),
    /// `[k]` on the repeat or collect just selected.
    Index(IndexPlan),
    /// Interface name `used[k]` of a choice: `$cc.x`, or `$s[1].x` after
    /// an index into a select's picks.
    Iface(ChoiceId, u32),
}

#[derive(Debug, Clone)]
enum IndexTerm {
    Const(i64),
    /// The position of the iteration `hops` template levels up from the
    /// referencing element.
    Iter(u32),
}

#[derive(Debug, Clone)]
struct IndexPlan {
    terms: Vec<IndexTerm>,
}

#[derive(Debug, Clone)]
struct RefPlan {
    /// Template levels up from the referencing element to where the first
    /// name was found.
    hops: u32,
    steps: Vec<Step>,
    /// The prop named by the final part, if any.
    prop: Option<String>,
    /// For error messages.
    display: String,
}

/// A planned element's prop list under construction: public slots set by
/// index, hidden slots appended after them and referenced by index like any
/// own prop.
struct ElemPlan {
    props: Vec<Option<SourcePlan>>,
    roles: HashMap<&'static str, u8>,
    /// The attribute a role's value came from, if any; a copy that gives
    /// that attribute itself does not share the role.
    role_attr: HashMap<&'static str, &'static str>,
}

impl ElemPlan {
    fn new(n_public: usize) -> Self {
        ElemPlan { props: vec![None; n_public], roles: HashMap::new(), role_attr: HashMap::new() }
    }
    fn set(&mut self, i: usize, plan: SourcePlan) {
        self.props[i] = Some(plan);
    }
    fn hidden(&mut self, plan: SourcePlan) -> u8 {
        self.props.push(Some(plan));
        u8::try_from(self.props.len() - 1).expect("fewer than 256 slots per element")
    }
    /// A hidden essential slot with a role name (see `Elem::roles`).
    fn essential(&mut self, role: &'static str, value: f64) -> u8 {
        let i = self.hidden(SourcePlan::Literal(value));
        self.roles.insert(role, i);
        i
    }
    /// A public essential slot with a role name.
    fn set_essential(&mut self, i: usize, role: &'static str, value: f64) {
        self.set(i, SourcePlan::Literal(value));
        self.roles.insert(role, i as u8);
    }
    /// Record that `role` came from attribute `attr`.
    fn from_attr(&mut self, role: &'static str, attr: &'static str) {
        self.role_attr.insert(role, attr);
    }
    fn finish(self) -> Vec<SourcePlan> {
        self.props.into_iter().enumerate().map(|(i, p)| p.unwrap_or_else(|| panic!("public prop {i} left unplanned"))).collect()
    }
}

enum CenterPlan {
    Ref(PlanId),
    Tuple(Vec<SourcePlan>),
}

/// Role names of the k-th literal point of a point list (k < 16).
const POINT_ROLES: [[&str; 2]; 16] = [
    ["pt1x", "pt1y"],
    ["pt2x", "pt2y"],
    ["pt3x", "pt3y"],
    ["pt4x", "pt4y"],
    ["pt5x", "pt5y"],
    ["pt6x", "pt6y"],
    ["pt7x", "pt7y"],
    ["pt8x", "pt8y"],
    ["pt9x", "pt9y"],
    ["pt10x", "pt10y"],
    ["pt11x", "pt11y"],
    ["pt12x", "pt12y"],
    ["pt13x", "pt13y"],
    ["pt14x", "pt14y"],
    ["pt15x", "pt15y"],
    ["pt16x", "pt16y"],
];

/// One point of a point-list attribute (`through`, `vertices`, `endpoints`).
#[derive(Debug, Clone)]
enum PointPlan {
    /// `$p`: a point-valued reference (two cells).
    Ref(PlanId),
    /// `(a, b)`: two scalar plans.
    Tuple([SourcePlan; 2]),
    /// Item `i` of an array prop: `$l.points` contributes one per item.
    Item(PlanId, usize),
}

impl PointPlan {
    fn coord(&self, j: usize) -> SourcePlan {
        match self {
            PointPlan::Ref(p) => SourcePlan::coord(*p, j),
            PointPlan::Tuple(xy) => xy[j].clone(),
            PointPlan::Item(p, i) => SourcePlan::item(*p, *i, j),
        }
    }
}

struct Compiled<'a> {
    dast: &'a Dast,
    templates: Vec<Template>,
    plans: Vec<RefPlan>,
    /// Expression templates: cell leaves hold plan ids.
    arena: Arena,
    /// The math text of templates that may be symbolic, with each `$ref`
    /// written `#plan` (see `cells_sym::parse`).
    sym_text: HashMap<ExprId, String>,
    choices: Vec<ChoiceDef>,
}

struct Compiler<'a> {
    c: Compiled<'a>,
    /// Elements whose attribute plans are computed once every name exists.
    pending_elems: Vec<(TemplateId, ElemId)>,
    /// Macro children awaiting plans: (template, owning element or None for
    /// the template's own children, index in that child list, node).
    pending_macros: Vec<(TemplateId, Option<ElemId>, usize, NodeId)>,
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
    scopes: Vec<(ScopeId, NodeId, u32)>,
    scope_index: HashMap<(ScopeId, NodeId, u32), ScopeId>,
    counts: HashMap<(ScopeId, NodeId), u32>,
    /// The document seed load-time choices draw from (plan 6).
    pub seed: u64,
    /// `values[scope][template slot]`
    values: Vec<Vec<Option<f64>>>,
}

impl Prior {
    /// Build a prior from a document, moving its value store out (the
    /// document is about to be replaced). `restore` puts it back on error.
    pub fn take_from(doc: &mut Document) -> Prior {
        let counts = doc.structure.repeats.iter().map(|r| ((r.scope, r.node), doc.repeat_count(r))).collect();
        let mut values = std::mem::take(&mut doc.structure.values);
        values.resize(doc.structure.scopes.len(), Vec::new());
        for (&(scope, slot), &v) in doc.structure.essential_slots.iter().zip(&doc.cells[..doc.n_essential]) {
            let row = &mut values[scope as usize];
            if row.len() <= slot as usize {
                row.resize(slot as usize + 1, None);
            }
            row[slot as usize] = Some(v);
        }
        Prior { scopes: doc.structure.scopes.clone(), scope_index: doc.structure.scope_index.clone(), counts, values, seed: doc.structure.seed }
    }

    pub fn restore(self, doc: &mut Document) {
        doc.structure.values = self.values;
    }
}

// ---------------------------------------------------------------------------
// Expansion
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum Source {
    Unset,
    Literal(f64),
    Default(f64),
    /// A constant that is not essential: an iteration index, a collect's
    /// count, a math handle, the shared missing-referent cell.
    Fixed(f64),
    Alias(SlotId),
    /// Operator over `op_inputs[start..start + n]`.
    Op(OpSpec, u32, u8),
    /// Head (output 0) of a vector instruction over `op_inputs[start..start + n]`.
    Vec(VecOp, u32, u8),
    /// Output `k` of the vector instruction headed at `head`.
    VecOut(SlotId, u8),
}

/// A built document whose program has not yet been scheduled.
pub struct Unscheduled {
    cells: Vec<f64>,
    n_essential: usize,
    n_fixed: usize,
    instrs: Vec<Instr>,
    comps: Components,
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
/// iterations. `Document::load_timed` iterates this to a fixed point.
pub fn build_once(dast: &Dast, engine: &mut dyn SymEngine) -> Result<Unscheduled> {
    build(dast, &Prior::default(), engine)
}

/// Where a reference path has arrived after walking its steps.
#[derive(Debug, Clone, Copy)]
enum Resolved {
    Comp(CompIdx),
    /// An iteration of a repeat, named by `$r[k]`.
    Iter(CompIdx, ScopeId),
    /// An index with no referent (`$r[32]` with ten iterations).
    Missing,
}

/// One expanded choice.
#[derive(Debug, Clone)]
struct ChoiceInst {
    def: ChoiceId,
    comp: CompIdx,
    /// The scope of each built branch (a select's picks in order; every
    /// case of a reactive choice).
    scopes: Vec<ScopeId>,
    /// The branch each of `scopes` instantiates.
    branch_of: Vec<usize>,
    /// Reactive choice: one `Choose` component per `ChoiceDef::used`.
    iface_comps: Vec<CompIdx>,
}

/// One instantiated template element.
#[derive(Debug, Clone, Copy)]
struct Instance {
    scope: ScopeId,
    template: TemplateId,
    elem: ElemId,
    comp: CompIdx,
}

struct Builder<'c, 'a> {
    c: &'c Compiled<'a>,
    prior: &'c Prior,
    engine: &'c mut dyn SymEngine,
    /// Templates of `Instantiate` instructions, cell leaves holding slots
    /// until emit rebinds them to cells and imports them.
    sym_templates: Vec<Tree>,
    /// Slots that hold expression handles without being an instruction's
    /// output (essential and fixed math cells).
    math_slots: Vec<SlotId>,
    /// Per component: 0 not yet known, 1 numeric, 2 symbolic, 3 deciding.
    symbolic: Vec<u8>,
    comps: Components,
    slot_base: Vec<u32>,
    sources: Vec<Source>,
    /// Owning component of each slot (prop index = slot - slot_base[comp]).
    slot_comp: Vec<CompIdx>,
    op_inputs: Vec<SlotId>,
    /// Scope table: (parent, repeat element, position); carried over and extended.
    scopes: Vec<(ScopeId, NodeId, u32)>,
    scope_index: HashMap<(ScopeId, NodeId, u32), ScopeId>,
    /// Per scope: element -> component, for the scope's template.
    scope_comps: Vec<Vec<CompIdx>>,
    /// Per component: index into `instances`, or NONE for synthesized ones.
    comp_instance: Vec<u32>,
    instances: Vec<Instance>,
    /// Per component: index into `repeats` for a repeat component.
    comp_repeat: Vec<u32>,
    repeats: Vec<Repeat>,
    counts_used: Vec<u32>,
    /// `$ref` children awaiting a kind: (component, plan, scope, has index).
    pending: Vec<(CompIdx, PlanId, ScopeId, bool)>,
    /// Collect components awaiting expansion.
    collects: Vec<CompIdx>,
    /// Point lists awaiting their synthesized children.
    pointlists: Vec<CompIdx>,
    collected: HashMap<CompIdx, Vec<CompIdx>>,
    /// Expanded choices, and the instance each choice component owns.
    choice_insts: Vec<ChoiceInst>,
    comp_choice: HashMap<CompIdx, usize>,
    missing: Option<SlotId>,
    arena: Arena,
    root: CompIdx,
}


/// `fixed`: the element's essential values become constants.
/// How a `fixed` attribute (and a graph's `fixAxes`) reaches an element.
enum Fix {
    Off,
    /// A literal true: the element's essential cells become fixed cells.
    Literal,
    /// References: flag cells (any nonzero holds) that gate the element.
    Dynamic(Vec<SourcePlan>),
}

/// Put a `Gate` on every slot a request could write through: its essential
/// literals, its references and its operators and math over other cells.
/// Each such plan moves to a new hidden slot and its old slot becomes
/// `Gate(moved, flag)`, so everything that reads the slot, inside the
/// element or out, reads through the gate. Vector heads and outputs stay in
/// place (they must be consecutive); their inputs are own slots, which are
/// gated themselves.
fn gate_slots(props: &mut Vec<Option<SourcePlan>>, flags: Vec<SourcePlan>) {
    let n = props.len();
    let push = |props: &mut Vec<Option<SourcePlan>>, plan: SourcePlan| {
        props.push(Some(plan));
        u8::try_from(props.len() - 1).expect("fewer than 256 slots per element")
    };
    let mut flags = flags.into_iter();
    let mut flag = push(props, flags.next().expect("a dynamic fix has a flag"));
    for f in flags {
        let g = push(props, f);
        flag = push(props, SourcePlan::Op(OpSpec::Max, vec![Arg::Own(flag), Arg::Own(g)]));
    }
    for i in 0..n {
        let moved = match &props[i] {
            Some(SourcePlan::Inherit) => SourcePlan::InheritFrom(i as u8),
            Some(SourcePlan::Literal(_) | SourcePlan::Default(_) | SourcePlan::Alias(Arg::Ref(..) | Arg::Elem(..)) | SourcePlan::Math(_)) => props[i].take().unwrap(),
            Some(SourcePlan::Op(_, args)) if args.iter().any(|a| !matches!(a, Arg::Own(_))) => props[i].take().unwrap(),
            _ => continue,
        };
        let h = push(props, moved);
        props[i] = Some(SourcePlan::Op(OpSpec::Gate, vec![Arg::Own(h), Arg::Own(flag)]));
    }
}

/// A literal `fixed`: essential cells, given or defaulted, become constants.
fn fix_literals(props: &mut [Option<SourcePlan>]) {
    for p in props.iter_mut() {
        if let Some(SourcePlan::Literal(v) | SourcePlan::Default(v)) = p {
            *p = Some(SourcePlan::Fixed(*v));
        }
    }
}

/// Fill in the kind and prop of an arity error raised by `resolve_one`.
fn arity_error(e: Error, kind: ComponentKind, prop: &str) -> Error {
    match e {
        Error::ArityMismatch { expected, got, .. } => Error::ArityMismatch { kind: kind.tag().into(), prop: prop.into(), expected, got },
        other => other,
    }
}

struct UnionFind {
    parent: Vec<u32>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        UnionFind { parent: (0..n as u32).collect() }
    }
    fn find(&mut self, mut x: u32) -> u32 {
        while self.parent[x as usize] != x {
            let p = self.parent[x as usize];
            self.parent[x as usize] = self.parent[p as usize];
            x = p;
        }
        x
    }
    fn union(&mut self, a: u32, b: u32) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.parent[ra as usize] = rb;
        }
    }
}
