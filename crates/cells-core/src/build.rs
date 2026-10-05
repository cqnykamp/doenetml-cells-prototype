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
//! Then, as before: union-find over aliases makes cells; cells are numbered
//! (essential, then fixed, then derived); operators are bound; and the
//! instruction list is scheduled, with a fast path when creation order is
//! already a valid evaluation order.

use std::collections::HashMap;

use crate::components::{ComponentKind, PropFrom};
use crate::dast::{Dast, NodeId, NodeKind, StrId, StringTable};
use crate::document::{CellIdx, CompIdx, Components, Document, NONE, Repeat, ScopeId, Structure, TEXT_BIT};
use crate::error::{Error, Result};
use crate::expr::{Arena, Expr, ExprId, Parser, Token};
use crate::geo::{Pivot, RigidOpts, VecOp};
use crate::ops::{Instr, OpSpec};
use crate::program::Program;

type SlotId = u32;
type TemplateId = usize;
type ElemId = usize;
type PlanId = usize;

// ---------------------------------------------------------------------------
// Compiled templates
// ---------------------------------------------------------------------------

/// Where a prop's value comes from, before any iteration exists.
#[derive(Debug, Clone)]
enum SourcePlan {
    Literal(f64),
    /// The default, or the matching prop of the `extend` referent.
    Default(f64),
    Fixed(f64),
    /// Alias of the single cell a reference names.
    Ref(PlanId),
    /// Alias of the `i`th of `n` cells a multi-cell reference names.
    RefPart(PlanId, usize, usize),
    /// `<op>`: operator over references.
    Op(OpSpec, Vec<PlanId>),
    /// Operator over the component's own props.
    Computed(OpSpec, Vec<u8>),
    /// Alias of the component's own prop.
    AliasOwn(u8),
    /// The 1-based position of the enclosing iteration.
    IterIndex,
    /// Math text: lowered to operators if numeric, else NaN.
    Math(ExprId),
    /// Head of a vector instruction over the component's own slots; this
    /// slot is output 0, the next `n_out - 1` slots are `VecOut`.
    Vec(VecOp, Vec<u8>),
    /// Output `k` of the vector instruction headed at own slot `head`.
    VecOut(u8, u8),
    /// Alias of coordinate `j` of item `i` of an array prop a reference
    /// names (`$l.points`).
    RefItem(PlanId, usize, usize),
    /// Alias of slot `slot` of element `elem` in the same template: a copy
    /// with overridden attributes shares the rest of the original's state.
    AliasElem(ElemId, u8),
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
    /// `<math>`: `expr` is the handle, `value` lowers or evaluates.
    Math(ExprId),
    /// `<pointList extend="$l.points">`: children are synthesized points.
    PointList {
        from: PlanId,
    },
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

/// Builds a planned element's prop list: public slots set by index, hidden
/// slots appended after them and referenced by index like any own prop.
struct Chain {
    props: Vec<Option<SourcePlan>>,
    roles: HashMap<&'static str, u8>,
    /// The attribute a role's value came from, if any; a copy that gives
    /// that attribute itself does not share the role.
    role_attr: HashMap<&'static str, &'static str>,
}

impl Chain {
    fn new(n_public: usize) -> Self {
        Chain { props: vec![None; n_public], roles: HashMap::new(), role_attr: HashMap::new() }
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
            PointPlan::Ref(p) => SourcePlan::RefPart(*p, j, 2),
            PointPlan::Tuple(xy) => xy[j].clone(),
            PointPlan::Item(p, i) => SourcePlan::RefItem(*p, *i, j),
        }
    }
}

struct Compiled<'a> {
    dast: &'a Dast,
    templates: Vec<Template>,
    plans: Vec<RefPlan>,
    /// Expression templates: cell leaves hold plan ids.
    arena: Arena,
}

struct Compiler<'a> {
    c: Compiled<'a>,
    /// Elements whose attribute plans are computed once every name exists.
    pending_elems: Vec<(TemplateId, ElemId)>,
    /// Macro children awaiting plans: (template, owning element or None for
    /// the template's own children, index in that child list, node).
    pending_macros: Vec<(TemplateId, Option<ElemId>, usize, NodeId)>,
}

impl<'a> Compiler<'a> {
    fn compile(dast: &'a Dast) -> Result<Compiled<'a>> {
        let mut cp = Compiler { c: Compiled { dast, templates: vec![Template::default()], plans: Vec::new(), arena: Arena::default() }, pending_elems: Vec::new(), pending_macros: Vec::new() };
        // Template 0 is the document: its one child is the root component.
        let doc_el = dast.children(Dast::ROOT).iter().copied().find(|&n| dast.kind(n) == NodeKind::Element && dast.str(n) == "document");
        let children = match doc_el {
            Some(el) => vec![Child::Elem(cp.add_elem(0, el, ROOT_SCOPE)?)],
            None => {
                // Synthesize a root when the DAST was not normalized.
                let e = cp.add_synthetic(0, ComponentKind::Document, NONE);
                let kids = cp.add_children(0, Some(e), ROOT_SCOPE, dast.children(Dast::ROOT))?;
                cp.c.templates[0].elems[e].children = kids;
                vec![Child::Elem(e)]
            }
        };
        cp.c.templates[0].children = children;
        // Every name exists now: plan attributes and macro children. Planning
        // a container copy adds elements, so loop until nothing is pending.
        while !cp.pending_elems.is_empty() || !cp.pending_macros.is_empty() {
            for (t, e) in std::mem::take(&mut cp.pending_elems) {
                cp.plan_elem(t, e)?;
            }
            for (t, owner, i, m) in std::mem::take(&mut cp.pending_macros) {
                let scope = match owner {
                    Some(e) => cp.child_scope(t, e),
                    None => ROOT_SCOPE,
                };
                let plan = cp.plan_ref(t, scope, m)?;
                let has_index = dast.macro_has_index(m);
                match owner {
                    Some(e) => cp.c.templates[t].elems[e].children[i] = Child::Macro(plan, has_index),
                    None => cp.c.templates[t].children[i] = Child::Macro(plan, has_index),
                }
            }
        }
        // Slot offsets, now that planned kinds know their hidden slot count.
        for tpl in &mut cp.c.templates {
            let mut off = 0;
            for el in &mut tpl.elems {
                el.slot_off = off;
                off += el.props.len().max(el.kind.prop_defs().len()) as u32;
            }
            tpl.n_slots = off;
        }
        Ok(cp.c)
    }

    /// Children of element `e` have `e` as their parent.
    fn child_scope(&self, _t: TemplateId, e: ElemId) -> ElemId {
        e
    }

    fn push_elem(&mut self, t: TemplateId, node: NodeId, kind: ComponentKind, name: StrId, name_scope: ElemId) -> Result<ElemId> {
        self.push_elem_visible_to(t, node, kind, name, name_scope, ROOT_SCOPE)
    }

    /// `push_elem` whose name is registered only up to ancestor `stop`
    /// (inclusive): a container copy's children are reached through the
    /// copy's name, never bare, so they do not make the original ambiguous.
    fn push_elem_visible_to(&mut self, t: TemplateId, node: NodeId, kind: ComponentKind, name: StrId, name_scope: ElemId, stop: ElemId) -> Result<ElemId> {
        let tpl = &mut self.c.templates[t];
        let e = tpl.elems.len();
        tpl.elems.push(Elem { node, kind, name, name_scope, slot_off: 0, props: Vec::new(), children: Vec::new(), extend: None, cloned: false, roles: HashMap::new(), body: Body::Plain });
        if name != NONE {
            let s = self.c.dast.strings.get(name).trim().to_string();
            // Two siblings with one name can never be told apart.
            if tpl.names.get(&(name_scope, s.clone())).is_some_and(|v| v.iter().any(|&o| tpl.elems[o].name_scope == name_scope)) {
                return Err(Error::DuplicateName(s));
            }
            // Visible from every ancestor up to the template root (or `stop`).
            let mut a = name_scope;
            loop {
                tpl.names.entry((a, s.clone())).or_default().push(e);
                if a == ROOT_SCOPE || a == stop {
                    break;
                }
                a = tpl.elems[a].name_scope;
            }
        }
        Ok(e)
    }

    fn add_synthetic(&mut self, t: TemplateId, kind: ComponentKind, name: StrId) -> ElemId {
        self.push_elem(t, NONE, kind, name, ROOT_SCOPE).expect("synthetic names are unique")
    }

    fn attr_name_str(&self, el: NodeId, attr: &str) -> Option<StrId> {
        let d = self.c.dast;
        let a = d.attr(el, attr)?;
        match d.attr_children(a) {
            [t] if d.kind(*t) == NodeKind::Text => Some(d.str_id(*t)),
            _ => None,
        }
    }

    /// Create an element (and, for a repeat, its template) from a DAST element.
    fn add_elem(&mut self, t: TemplateId, el: NodeId, name_scope: ElemId) -> Result<ElemId> {
        let d = self.c.dast;
        let tag = d.str(el);
        let kind = ComponentKind::from_tag(tag).ok_or_else(|| Error::UnsupportedTag(tag.to_string()))?;
        let name = self.attr_name_str(el, "name").unwrap_or(NONE);
        let e = self.push_elem(t, el, kind, name, name_scope)?;
        self.pending_elems.push((t, e));
        let child_scope = e;
        let _ = name_scope;
        match kind {
            ComponentKind::RepeatForSequence => {
                let sub = self.c.templates.len();
                self.c.templates.push(Template { parent: Some((t, e)), ..Default::default() });
                // Hidden iteration components first, so `$v` and `$i` resolve
                // anywhere inside the template.
                if let Some(vn) = self.attr_name_str(el, "valueName") {
                    let v = self.add_synthetic(sub, ComponentKind::SequenceValue, vn);
                    // from and step alias the repeat's own props (one hop up);
                    // k is the iteration position; the rest is the kind's chain.
                    let from = self.own_prop_plan(e, "from");
                    let step = self.own_prop_plan(e, "step");
                    let mut props = vec![SourcePlan::Ref(from), SourcePlan::Ref(step), SourcePlan::IterIndex];
                    for def in ComponentKind::SequenceValue.prop_defs().iter().skip(3) {
                        let PropFrom::Computed { op, args } = def.from else { unreachable!() };
                        props.push(SourcePlan::Computed(op, args.to_vec()));
                    }
                    self.c.templates[sub].elems[v].props = props;
                }
                if let Some(iname) = self.attr_name_str(el, "indexName") {
                    let i = self.add_synthetic(sub, ComponentKind::Number, iname);
                    self.c.templates[sub].elems[i].props = vec![SourcePlan::IterIndex];
                }
                let kids = self.add_children(sub, None, ROOT_SCOPE, d.children(el))?;
                self.c.templates[sub].children = kids;
                self.c.templates[t].elems[e].body = Body::Repeat { template: sub };
            }
            ComponentKind::Collect | ComponentKind::PointList => {}
            // A number's children are its value, not rendered children; the
            // planned kinds read their children themselves (a line's equation,
            // a point's constraints).
            _ if kind.planned() || kind == ComponentKind::Point || kind.prop_defs().iter().any(|p| p.from == PropFrom::Children) => {}
            _ => {
                let kids = self.add_children(t, Some(e), child_scope, d.children(el))?;
                self.c.templates[t].elems[e].children = kids;
            }
        }
        Ok(e)
    }

    /// A plan naming prop `prop` of element `e` in the parent template, as
    /// seen from the repeat's own template (one hop up).
    fn own_prop_plan(&mut self, e: ElemId, prop: &str) -> PlanId {
        self.c.plans.push(RefPlan { hops: 1, steps: vec![Step::Elem(e)], prop: Some(prop.to_string()), display: format!("(repeat).{prop}") });
        self.c.plans.len() - 1
    }

    fn add_children(&mut self, t: TemplateId, owner: Option<ElemId>, name_scope: ElemId, nodes: &[NodeId]) -> Result<Vec<Child>> {
        let d = self.c.dast;
        let mut kids = Vec::with_capacity(nodes.len());
        for &n in nodes {
            match d.kind(n) {
                NodeKind::Element => {
                    let tag = d.str(n);
                    // Normalizer-synthesized elements: `_repeatSetup` holds
                    // placeholders the expansion creates itself.
                    if tag == "_dynamicChildren" || tag == "_repeatSetup" {
                        continue;
                    }
                    kids.push(Child::Elem(self.add_elem(t, n, name_scope)?));
                }
                // Whitespace-only text carries no content and would become a DOM node.
                NodeKind::Text if d.str(n).trim().is_empty() => {}
                NodeKind::Text => kids.push(Child::Text(d.str_id(n))),
                NodeKind::Macro => {
                    self.pending_macros.push((t, owner, kids.len(), n));
                    kids.push(Child::Macro(usize::MAX, false));
                }
                NodeKind::Other => {}
            }
        }
        Ok(kids)
    }

    // ---- attribute plans ----------------------------------------------------

    fn plan_elem(&mut self, t: TemplateId, e: ElemId) -> Result<()> {
        let d = self.c.dast;
        let (el, kind, scope, cloned) = {
            let x = &self.c.templates[t].elems[e];
            (x.node, x.kind, x.name_scope, x.cloned)
        };
        if cloned {
            // Every public prop aliases the original's (`Default` under an
            // extend); nothing else to plan.
            let n = kind.prop_defs().len();
            self.c.templates[t].elems[e].props = (0..n).map(|_| SourcePlan::Default(f64::NAN)).collect();
            return Ok(());
        }
        if el == NONE {
            return Ok(());
        }
        let mut extend_prop: Option<PlanId> = None;
        let extend = match d.attr(el, "extend") {
            Some(a) => {
                let m = self.single_macro(a).ok_or_else(|| Error::BadValue { attr: "extend".into(), text: self.attr_text(a).unwrap_or_default() })?;
                let p = self.plan_ref(t, scope, m)?;
                if self.c.plans[p].prop.is_some() {
                    // `<point extend="$c.center"/>`, `<math extend="$c.radius"/>`,
                    // `<pointList extend="$l.points"/>`: the element's value
                    // props alias the named prop.
                    extend_prop = Some(p);
                    None
                } else {
                    Some(p)
                }
            }
            None => None,
        };
        self.c.templates[t].elems[e].extend = extend;

        if let Some(p) = extend_prop {
            return self.plan_extend_prop(t, e, p);
        }
        if kind.container()
            && let Some(p) = extend
        {
            return self.plan_container_copy(t, e, p);
        }
        if kind == ComponentKind::PointList {
            return Err(Error::BadValue { attr: "extend".into(), text: "<pointList> needs extend=\"$shape.points\"".into() });
        }
        if kind.planned() {
            return self.plan_geo(t, e, extend);
        }

        match kind {
            ComponentKind::Math => {
                // A tuple-valued math (`<math>(a, b)</math>`) is a point for
                // the cells core: two cells, draggable as a direction source.
                let nodes: Vec<NodeId> = d.children(el).to_vec();
                let (toks, _) = self.math_tokens(t, scope, &nodes)?;
                if let Some(inner) = crate::expr::unwrap_parens(&toks)
                    && crate::expr::split_top(inner, &Token::Comma).len() == 2
                {
                    let xy = self.plan_tuple(t, scope, &nodes)?;
                    let hide = ComponentKind::Point.prop_defs()[2].default;
                    self.c.templates[t].elems[e].kind = ComponentKind::Point;
                    self.c.templates[t].elems[e].props = vec![xy[0].clone(), xy[1].clone(), SourcePlan::Default(hide)];
                    return Ok(());
                }
                let id = self.plan_math(t, scope, &nodes)?;
                self.c.templates[t].elems[e].body = Body::Math(id);
                return Ok(());
            }
            ComponentKind::Point if extend.is_none() && d.attr(el, "x").is_none() && d.attr(el, "y").is_none() && d.attr(el, "coords").is_none() && !d.children(el).iter().all(|&n| self.is_blank(n)) => {
                return self.plan_point_children(t, e);
            }
            ComponentKind::Point if d.children(el).iter().any(|&n| d.kind(n) == NodeKind::Element) => {
                // Children are constraints; coordinates come from attributes.
                self.plan_point_attrs(t, e, extend)?;
                return self.plan_point_constraints(t, e);
            }
            ComponentKind::Collect => {
                let from = d.attr(el, "from").and_then(|a| self.single_macro(a)).ok_or(Error::BadCollect)?;
                let type_text = d.attr(el, "componentType").and_then(|a| self.attr_text(a)).ok_or(Error::BadCollect)?;
                let ck = ComponentKind::from_tag(type_text.trim()).filter(|k| k.collectable()).ok_or_else(|| Error::BadCollectType(type_text.trim().into()))?;
                let p = self.plan_ref(t, scope, from)?;
                if self.c.plans[p].prop.is_some() {
                    return Err(Error::BadCollect);
                }
                self.c.templates[t].elems[e].body = Body::Collect { from: p, kind: ck };
                // `count` is set when the collect expands.
                self.c.templates[t].elems[e].props = vec![SourcePlan::Fixed(f64::NAN)];
                return Ok(());
            }
            _ => {}
        }

        self.plan_attrs(t, e, extend)
    }

    /// Plans for a kind described by `PropFrom`: attributes, bindings,
    /// computed chains, children.
    fn plan_attrs(&mut self, t: TemplateId, e: ElemId, extend: Option<PlanId>) -> Result<()> {
        let d = self.c.dast;
        let (el, kind, scope) = {
            let x = &self.c.templates[t].elems[e];
            (x.node, x.kind, x.name_scope)
        };
        let _ = extend;
        let mut props: Vec<Option<SourcePlan>> = vec![None; kind.prop_defs().len()];
        // Virtual multi-cell attributes (a point's coords) bind several props at once.
        for (vname, parts) in virtual_attrs(kind) {
            if let Some(a) = d.attr(el, vname) {
                let m = self.single_macro(a).ok_or_else(|| Error::BadValue { attr: vname.into(), text: self.attr_text(a).unwrap_or_default() })?;
                let p = self.plan_ref(t, scope, m)?;
                for (i, part) in parts.iter().enumerate() {
                    if d.attr(el, part).is_some() {
                        return Err(Error::BadValue { attr: part.to_string(), text: format!("conflicts with {vname}") });
                    }
                    props[kind.prop_index(part).unwrap()] = Some(SourcePlan::RefPart(p, i, parts.len()));
                }
            }
        }
        for (pi, def) in kind.prop_defs().iter().enumerate() {
            if props[pi].is_some() {
                continue;
            }
            let bound = match def.bind.and_then(|b| d.attr(el, b)) {
                Some(a) => {
                    let bind = def.bind.unwrap();
                    let m = self.single_macro(a).ok_or_else(|| Error::BadValue { attr: bind.into(), text: self.attr_text(a).unwrap_or_default() })?;
                    Some(SourcePlan::Ref(self.plan_ref(t, scope, m)?))
                }
                None => None,
            };
            let plan = match (bound, def.from) {
                (Some(p), _) => p,
                (None, PropFrom::Attribute) => match d.attr(el, def.attr_name()) {
                    Some(a) => self.plan_value(t, scope, def.attr_name(), d.attr_children(a), def.ref_prop)?,
                    None => SourcePlan::Default(def.default),
                },
                (None, PropFrom::AttributeOr { alias }) => match d.attr(el, def.attr_name()) {
                    Some(a) => self.plan_value(t, scope, def.attr_name(), d.attr_children(a), None)?,
                    None => SourcePlan::AliasOwn(alias),
                },
                (None, PropFrom::Computed { op, args }) => SourcePlan::Computed(op, args.to_vec()),
                (None, PropFrom::Children) => {
                    let blank = d.children(el).iter().all(|&n| self.is_blank(n));
                    // A mathInput's `prefill` stands in for blank children.
                    let from_attr = def.attr.and_then(|a| d.attr(el, a));
                    if blank {
                        match from_attr {
                            Some(a) => match self.plan_value(t, scope, def.attr_name(), d.attr_children(a), None) {
                                Ok(p) => p,
                                Err(Error::BadValue { .. }) => SourcePlan::Math(self.plan_math(t, scope, d.attr_children(a))?),
                                Err(e) => return Err(e),
                            },
                            None => SourcePlan::Default(def.default),
                        }
                    } else {
                        match self.plan_value(t, scope, def.name, d.children(el), None) {
                            // `<number>3</number>` cannot be changed by a drag in
                            // the current core (no math child to write), so it is
                            // a constant; an input's literal is its initial state.
                            Ok(SourcePlan::Literal(v)) if kind == ComponentKind::Number => SourcePlan::Fixed(v),
                            Ok(p) => p,
                            // Not a single literal or reference: math text.
                            Err(Error::BadValue { .. }) => SourcePlan::Math(self.plan_math(t, scope, d.children(el))?),
                            Err(e) => return Err(e),
                        }
                    }
                }
                (None, PropFrom::Derived) => self.plan_op(t, scope, el)?,
                (None, PropFrom::Planned) => unreachable!("planned kinds take plan_geo"),
            };
            props[pi] = Some(plan);
        }
        let mut props: Vec<Option<SourcePlan>> = props;
        if self.attr_flag(el, "fixed") {
            fix_literals(&mut props);
        }
        self.c.templates[t].elems[e].props = props.into_iter().map(|p| p.unwrap()).collect();
        Ok(())
    }

    /// A literal number or a single reference.
    fn plan_value(&mut self, t: TemplateId, scope: ElemId, attr: &str, nodes: &[NodeId], ref_prop: Option<&str>) -> Result<SourcePlan> {
        let d = self.c.dast;
        let macros: Vec<NodeId> = nodes.iter().copied().filter(|&n| d.kind(n) == NodeKind::Macro).collect();
        let text: String = nodes.iter().filter(|&&n| d.kind(n) == NodeKind::Text).map(|&n| d.str(n)).collect();
        let text = text.trim();
        match (macros.len(), text.is_empty()) {
            (1, true) => {
                let p = self.plan_ref(t, scope, macros[0])?;
                if let (None, Some(rp)) = (&self.c.plans[p].prop, ref_prop) {
                    self.c.plans[p].prop = Some(rp.to_string());
                }
                Ok(SourcePlan::Ref(p))
            }
            (0, false) => match text {
                "true" => Ok(SourcePlan::Literal(1.0)),
                "false" => Ok(SourcePlan::Literal(0.0)),
                _ => text.parse::<f64>().map(SourcePlan::Literal).map_err(|_| Error::BadValue { attr: attr.into(), text: text.into() }),
            },
            _ => Err(Error::BadValue { attr: attr.into(), text: text.into() }),
        }
    }

    fn plan_op(&mut self, t: TemplateId, scope: ElemId, el: NodeId) -> Result<SourcePlan> {
        let d = self.c.dast;
        let kind_text = d.attr(el, "kind").and_then(|a| self.attr_text(a)).unwrap_or_default();
        let kind_text = kind_text.trim();
        let param = |name: &str| -> Result<f64> {
            let a = d.attr(el, name).ok_or_else(|| Error::MissingParam { kind: kind_text.into(), attr: name.into() })?;
            self.attr_text(a).and_then(|t| t.trim().parse::<f64>().ok()).ok_or_else(|| Error::BadLiteralParam(name.into()))
        };
        let spec = match kind_text {
            "add" => OpSpec::Add,
            "sub" => OpSpec::Sub,
            "mul" => OpSpec::Mul,
            "div" => OpSpec::Div,
            "min" => OpSpec::Min,
            "max" => OpSpec::Max,
            "default" => OpSpec::Default,
            "negate" => OpSpec::Negate,
            "round" => OpSpec::Round,
            "floor" => OpSpec::Floor,
            "scale" => OpSpec::Scale { k: param("k")? },
            "offset" => OpSpec::Offset { k: param("k")? },
            "clamp" => OpSpec::Clamp { lo: param("lo")?, hi: param("hi")? },
            "nanTo" => OpSpec::NanTo { k: param("k")? },
            "lerp" => OpSpec::Lerp { t: param("t")? },
            other => return Err(Error::UnknownOp(other.into())),
        };
        let mut args = Vec::new();
        if let Some(a) = d.attr(el, "args") {
            for &node in d.attr_children(a) {
                match d.kind(node) {
                    NodeKind::Macro => args.push(self.plan_ref(t, scope, node)?),
                    NodeKind::Text if d.str(node).trim().is_empty() => {}
                    NodeKind::Text => return Err(Error::LiteralArg),
                    _ => {}
                }
            }
        }
        if args.len() != spec.arity() {
            return Err(Error::OpArity { kind: kind_text.into(), expected: spec.arity(), got: args.len() });
        }
        Ok(SourcePlan::Op(spec, args))
    }

    /// Math text and `$ref` children to an expression template whose cell
    /// leaves are plan ids.
    fn plan_math(&mut self, t: TemplateId, scope: ElemId, nodes: &[NodeId]) -> Result<ExprId> {
        let (toks, text) = self.math_tokens(t, scope, nodes)?;
        Parser::parse(&toks, &mut self.c.arena).map_err(|reason| Error::BadMath { text, reason })
    }

    /// Tokenize math text with `$ref` children as cell leaves holding plan ids.
    fn math_tokens(&mut self, t: TemplateId, scope: ElemId, nodes: &[NodeId]) -> Result<(Vec<Token>, String)> {
        let d = self.c.dast;
        let mut toks: Vec<Token> = Vec::new();
        let mut text = String::new();
        for &n in nodes {
            match d.kind(n) {
                NodeKind::Text => {
                    text.push_str(d.str(n));
                    crate::expr::tokenize(d.str(n), &mut toks).map_err(|reason| Error::BadMath { text: text.clone(), reason })?;
                }
                NodeKind::Macro => {
                    text.push('$');
                    text.push_str(&d.macro_display(n));
                    let p = self.plan_ref(t, scope, n)?;
                    toks.push(Token::Cell(p as CellIdx));
                }
                _ => {}
            }
        }
        Ok((toks, text.trim().to_string()))
    }

    // ---- planned kinds -------------------------------------------------------
    //
    // Geometric kinds are planned from the whole element: which attributes
    // are present decides which operator chain produces the public props.
    // That is where the current core's build-time variety goes (plan 3).

    /// A copy of plan `p` naming `prop` instead of its own (`$l` -> `$l.x2`).
    fn plan_with_prop(&mut self, p: PlanId, prop: &str) -> PlanId {
        let mut plan = self.c.plans[p].clone();
        plan.prop = Some(prop.to_string());
        plan.display = format!("{}.{prop}", plan.display);
        self.c.plans.push(plan);
        self.c.plans.len() - 1
    }

    /// The element a plan names, if it is in template `t` itself (a path of
    /// names without indices, no prop).
    fn plan_elem_target(&self, _t: TemplateId, p: PlanId) -> Option<ElemId> {
        let plan = &self.c.plans[p];
        if plan.hops != 0 || plan.prop.is_some() {
            return None;
        }
        let mut last = None;
        for step in &plan.steps {
            match step {
                Step::Elem(e) => last = Some(*e),
                Step::Index(_) => return None,
            }
        }
        last
    }

    /// A boolean attribute: present and empty, or `true`.
    fn attr_flag(&self, el: NodeId, name: &str) -> bool {
        match self.c.dast.attr(el, name) {
            None => false,
            Some(a) => {
                let text = self.attr_text(a).unwrap_or_default();
                let text = text.trim();
                text.is_empty() || text.eq_ignore_ascii_case("true")
            }
        }
    }

    /// The source for one scalar attribute value: literal, reference, or math.
    fn plan_scalar(&mut self, t: TemplateId, scope: ElemId, attr: &str, nodes: &[NodeId]) -> Result<SourcePlan> {
        match self.plan_value(t, scope, attr, nodes, None) {
            Ok(p) => Ok(p),
            Err(Error::BadValue { .. }) => {
                let (toks, text) = self.math_tokens(t, scope, nodes)?;
                let id = Parser::parse(&toks, &mut self.c.arena).map_err(|reason| Error::BadMath { text, reason })?;
                Ok(self.plan_from_expr(id))
            }
            Err(e) => Err(e),
        }
    }

    /// A plan from an expression template: a constant is an essential
    /// literal, a lone reference an alias, anything else a lowered math.
    fn plan_from_expr(&mut self, id: ExprId) -> SourcePlan {
        match self.c.arena.get(id) {
            Expr::Num(v) => SourcePlan::Literal(*v),
            Expr::Cell(p) => SourcePlan::Ref(*p as PlanId),
            _ => SourcePlan::Math(id),
        }
    }

    /// `(a, b)`: two scalar plans from tuple text, with `$ref` leaves.
    fn plan_tuple(&mut self, t: TemplateId, scope: ElemId, nodes: &[NodeId]) -> Result<Vec<SourcePlan>> {
        let d = self.c.dast;
        let macros: Vec<NodeId> = nodes.iter().copied().filter(|&n| d.kind(n) == NodeKind::Macro).collect();
        let text_blank = nodes.iter().all(|&n| d.kind(n) != NodeKind::Text || d.str(n).trim().is_empty());
        if macros.len() == 1 && text_blank {
            // `<point>$q</point>`: alias the referent's coordinates.
            let p = self.plan_ref(t, scope, macros[0])?;
            return Ok(vec![SourcePlan::RefPart(p, 0, 2), SourcePlan::RefPart(p, 1, 2)]);
        }
        let (toks, text) = self.math_tokens(t, scope, nodes)?;
        let inner = crate::expr::unwrap_parens(&toks).ok_or_else(|| Error::BadMath { text: text.clone(), reason: "expected a tuple like (x, y)".into() })?;
        let parts = crate::expr::split_top(inner, &Token::Comma);
        if parts.len() != 2 {
            return Err(Error::BadMath { text, reason: format!("expected 2 coordinates, got {}", parts.len()) });
        }
        let mut out = Vec::with_capacity(2);
        for part in parts {
            let id = Parser::parse(&part, &mut self.c.arena).map_err(|reason| Error::BadMath { text: text.clone(), reason })?;
            out.push(self.plan_from_expr(id));
        }
        Ok(out)
    }

    /// `<point>(2, $a)</point>`: coordinates from the children.
    fn plan_point_children(&mut self, t: TemplateId, e: ElemId) -> Result<()> {
        let d = self.c.dast;
        let (el, scope) = {
            let x = &self.c.templates[t].elems[e];
            (x.node, x.name_scope)
        };
        let nodes: Vec<NodeId> = d.children(el).iter().copied().filter(|&n| d.kind(n) != NodeKind::Element).collect();
        let xy = self.plan_tuple(t, scope, &nodes)?;
        let hide = ComponentKind::Point.prop_defs()[2].default;
        let mut props = vec![Some(xy[0].clone()), Some(xy[1].clone()), Some(SourcePlan::Default(hide))];
        if self.attr_flag(el, "fixed") {
            fix_literals(&mut props);
        }
        self.c.templates[t].elems[e].props = props.into_iter().map(|p| p.unwrap()).collect();
        if d.children(el).iter().any(|&n| d.kind(n) == NodeKind::Element) {
            self.plan_point_constraints(t, e)?;
        }
        Ok(())
    }

    fn plan_point_attrs(&mut self, t: TemplateId, e: ElemId, extend: Option<PlanId>) -> Result<()> {
        let d = self.c.dast;
        let (el, scope) = {
            let x = &self.c.templates[t].elems[e];
            (x.node, x.name_scope)
        };
        // `coords="(1,2)"` as a literal tuple.
        if let Some(a) = d.attr(el, "coords")
            && self.single_macro(a).is_none()
        {
            let xy = self.plan_tuple(t, scope, d.attr_children(a))?;
            let hide = ComponentKind::Point.prop_defs()[2].default;
            self.c.templates[t].elems[e].props = vec![xy[0].clone(), xy[1].clone(), SourcePlan::Default(hide)];
            return Ok(());
        }
        self.plan_attrs(t, e, extend)
    }

    /// Constraint children of a point: the planned coordinates move to
    /// hidden raw slots and the public `x`, `y` become their projection.
    fn plan_point_constraints(&mut self, t: TemplateId, e: ElemId) -> Result<()> {
        let d = self.c.dast;
        let (el, scope) = {
            let x = &self.c.templates[t].elems[e];
            (x.node, x.name_scope)
        };
        // Constraints may sit directly under the point or in <constraints>.
        let mut constraints: Vec<NodeId> = Vec::new();
        for &n in d.children(el) {
            if d.kind(n) == NodeKind::Element {
                if d.str(n) == "constraints" {
                    constraints.extend(d.children(n).iter().copied().filter(|&c| d.kind(c) == NodeKind::Element));
                } else {
                    constraints.push(n);
                }
            }
        }
        if constraints.is_empty() {
            return Ok(());
        }
        if constraints.len() > 1 {
            return Err(Error::Unsupported("more than one constraint on a point".into()));
        }
        let c = constraints[0];
        let props = std::mem::take(&mut self.c.templates[t].elems[e].props);
        let mut ch = Chain::new(props.len());
        let raw_x = ch.hidden(props[0].clone());
        let raw_y = ch.hidden(props[1].clone());
        ch.set(2, props[2].clone());
        match d.str(c) {
            "constrainToGrid" => {
                let num = |me: &Self, name: &str, default: f64| -> Result<f64> {
                    match d.attr(c, name) {
                        None => Ok(default),
                        Some(a) => me.attr_text(a).and_then(|s| s.trim().parse::<f64>().ok()).ok_or_else(|| Error::BadLiteralParam(name.into())),
                    }
                };
                let (dx, dy) = (num(self, "dx", 1.0)?, num(self, "dy", 1.0)?);
                let (xo, yo) = (num(self, "xoffset", 0.0)?, num(self, "yoffset", 0.0)?);
                let snap = |ch: &mut Chain, raw: u8, step: f64, offset: f64| -> SourcePlan {
                    let mut cur = raw;
                    if offset != 0.0 {
                        cur = ch.hidden(SourcePlan::Computed(OpSpec::Offset { k: -offset }, vec![cur]));
                    }
                    if step != 1.0 {
                        cur = ch.hidden(SourcePlan::Computed(OpSpec::Scale { k: 1.0 / step }, vec![cur]));
                    }
                    cur = ch.hidden(SourcePlan::Computed(OpSpec::Round, vec![cur]));
                    if step != 1.0 {
                        cur = ch.hidden(SourcePlan::Computed(OpSpec::Scale { k: step }, vec![cur]));
                    }
                    if offset != 0.0 {
                        return SourcePlan::Computed(OpSpec::Offset { k: offset }, vec![cur]);
                    }
                    SourcePlan::AliasOwn(cur)
                };
                let px = snap(&mut ch, raw_x, dx, xo);
                let py = snap(&mut ch, raw_y, dy, yo);
                ch.set(0, px);
                ch.set(1, py);
            }
            "constrainTo" => {
                let m = d.children(c).iter().copied().find(|&n| d.kind(n) == NodeKind::Macro).ok_or_else(|| Error::Unsupported("<constrainTo> without a reference".into()))?;
                let p = self.plan_ref(t, scope, m)?;
                let target = self.plan_elem_target(t, p).ok_or_else(|| Error::Unsupported("<constrainTo> must name a component in the same scope".into()))?;
                match self.c.templates[t].elems[target].kind {
                    ComponentKind::Circle => {
                        let cx = ch.hidden(SourcePlan::Ref(self.plan_with_prop(p, "cx")));
                        let cy = ch.hidden(SourcePlan::Ref(self.plan_with_prop(p, "cy")));
                        let r = ch.hidden(SourcePlan::Ref(self.plan_with_prop(p, "radius")));
                        ch.set(0, SourcePlan::Vec(VecOp::ProjectCircle, vec![raw_x, raw_y, cx, cy, r]));
                        ch.set(1, SourcePlan::VecOut(0, 1));
                    }
                    ComponentKind::Line | ComponentKind::LineSegment => {
                        let x1 = ch.hidden(SourcePlan::Ref(self.plan_with_prop(p, "x1")));
                        let y1 = ch.hidden(SourcePlan::Ref(self.plan_with_prop(p, "y1")));
                        let x2 = ch.hidden(SourcePlan::Ref(self.plan_with_prop(p, "x2")));
                        let y2 = ch.hidden(SourcePlan::Ref(self.plan_with_prop(p, "y2")));
                        ch.set(0, SourcePlan::Vec(VecOp::ProjectLine, vec![raw_x, raw_y, x1, y1, x2, y2]));
                        ch.set(1, SourcePlan::VecOut(0, 1));
                    }
                    other => {
                        return Err(Error::Unsupported(format!("constrainTo a <{}>", other.tag())));
                    }
                }
            }
            other => return Err(Error::Unsupported(format!("<{other}> constraint"))),
        }
        self.c.templates[t].elems[e].props = ch.finish();
        Ok(())
    }

    /// `extend="$c.prop"`: the element's value props alias the named prop.
    fn plan_extend_prop(&mut self, t: TemplateId, e: ElemId, p: PlanId) -> Result<()> {
        let kind = self.c.templates[t].elems[e].kind;
        let display = self.c.plans[p].display.clone();
        let props = match kind {
            ComponentKind::Point => {
                let hide = ComponentKind::Point.prop_defs()[2].default;
                vec![SourcePlan::RefPart(p, 0, 2), SourcePlan::RefPart(p, 1, 2), SourcePlan::Default(hide)]
            }
            ComponentKind::Number | ComponentKind::NumberInput | ComponentKind::MathInput => {
                vec![SourcePlan::Ref(p)]
            }
            ComponentKind::Math => {
                let id = self.c.arena.push(Expr::Cell(p as CellIdx));
                self.c.templates[t].elems[e].body = Body::Math(id);
                return Ok(());
            }
            ComponentKind::PointList => {
                self.c.templates[t].elems[e].body = Body::PointList { from: p };
                Vec::new()
            }
            _ => return Err(Error::PathTooDeep(display)),
        };
        self.c.templates[t].elems[e].props = props;
        Ok(())
    }

    /// `<graph extend="$g" name="g2"/>`: the copy's props alias the
    /// original's and its children are clones, named under the copy.
    fn plan_container_copy(&mut self, t: TemplateId, e: ElemId, p: PlanId) -> Result<()> {
        let r = self.plan_elem_target(t, p).ok_or_else(|| Error::UncopyableKind("graph from another scope".into()))?;
        self.plan_attrs(t, e, Some(p))?;
        let scope = self.child_scope(t, e);
        let kids = self.clone_children(t, r, scope, e)?;
        self.c.templates[t].elems[e].children = kids;
        Ok(())
    }

    fn clone_children(&mut self, t: TemplateId, from: ElemId, scope: ElemId, root: ElemId) -> Result<Vec<Child>> {
        let kids = self.c.templates[t].elems[from].children.clone();
        let mut out = Vec::with_capacity(kids.len());
        for ch in kids {
            match ch {
                Child::Elem(c) => {
                    let (kind, name) = {
                        let el = &self.c.templates[t].elems[c];
                        (el.kind, el.name)
                    };
                    if matches!(self.c.templates[t].elems[c].body, Body::Repeat { .. } | Body::Collect { .. }) {
                        return Err(Error::UncopyableKind(format!("{} inside a copied container", kind.tag())));
                    }
                    let ne = self.push_elem_visible_to(t, NONE, kind, name, scope, root)?;
                    let label = format!("(copy of {})", self.elem_label(t, c));
                    self.c.plans.push(RefPlan { hops: 0, steps: vec![Step::Elem(c)], prop: None, display: label });
                    let ep = self.c.plans.len() - 1;
                    {
                        let el = &mut self.c.templates[t].elems[ne];
                        el.extend = Some(ep);
                        el.cloned = true;
                    }
                    let child_scope = if name != NONE { ne } else { scope };
                    let sub = self.clone_children(t, c, child_scope, root)?;
                    self.c.templates[t].elems[ne].children = sub;
                    self.pending_elems.push((t, ne));
                    out.push(Child::Elem(ne));
                }
                other => out.push(other),
            }
        }
        Ok(out)
    }

    /// Dispatch for the geometric kinds.
    fn plan_geo(&mut self, t: TemplateId, e: ElemId, extend: Option<PlanId>) -> Result<()> {
        let d = self.c.dast;
        let (el, kind) = {
            let x = &self.c.templates[t].elems[e];
            (x.node, x.kind)
        };
        // Own attributes other than name and extend, or children, override
        // the referent's; with none, the copy aliases every public prop.
        let own_attrs: Vec<String> = d.attrs(el).map(|a| d.attr_name(a).to_string()).filter(|n| n != "name" && n != "extend").collect();
        let has_children = !d.children(el).iter().all(|&n| self.is_blank(n));
        if extend.is_some() && own_attrs.is_empty() && !has_children {
            let n = kind.prop_defs().len();
            self.c.templates[t].elems[e].props = (0..n).map(|_| SourcePlan::Default(f64::NAN)).collect();
            return Ok(());
        }
        // Merged attributes: the referent's node supplies what the copy omits.
        let base_elem = extend.and_then(|p| self.plan_elem_target(t, p)).filter(|&r| self.c.templates[t].elems[r].node != NONE);
        let base = base_elem.map(|r| self.c.templates[t].elems[r].node);
        let mut ch = match kind {
            ComponentKind::Circle => self.plan_circle(t, e, base)?,
            ComponentKind::Line => self.plan_line(t, e, base)?,
            ComponentKind::LineSegment => self.plan_segment(t, e, base)?,
            ComponentKind::Polygon => self.plan_polygon(t, e, base)?,
            _ => unreachable!(),
        };
        // Essential state the copy did not override is the original's: the
        // current core's copies share their essential state variables.
        if let Some(r) = base_elem {
            let base_roles = self.c.templates[t].elems[r].roles.clone();
            for (role, &slot) in &ch.roles {
                // A role the copy's own attribute supplied is its own state.
                if ch.role_attr.get(role).is_some_and(|a| own_attrs.iter().any(|o| o == a)) {
                    continue;
                }
                if let Some(&bs) = base_roles.get(role)
                    && matches!(ch.props[slot as usize], Some(SourcePlan::Literal(_)))
                {
                    ch.props[slot as usize] = Some(SourcePlan::AliasElem(r, bs));
                }
            }
        }
        if self.attr_flag(el, "fixed") {
            fix_literals(&mut ch.props);
        }
        self.c.templates[t].elems[e].roles = ch.roles.clone();
        self.c.templates[t].elems[e].props = ch.finish();
        Ok(())
    }

    /// Attribute of the element, falling back to the extend referent's.
    fn geo_attr(&self, el: NodeId, base: Option<NodeId>, name: &str) -> Option<u32> {
        let d = self.c.dast;
        d.attr(el, name).or_else(|| base.and_then(|b| d.attr(b, name)))
    }

    /// Two hidden slots holding a point. A literal coordinate is essential
    /// state and gets a role from `roles`, so a copy with overrides shares it.
    fn point_slots(ch: &mut Chain, p: &PointPlan) -> [u8; 2] {
        [ch.hidden(p.coord(0)), ch.hidden(p.coord(1))]
    }

    /// Put a point's coordinates on two public slots, tagging literal
    /// coordinates with roles. A free shape's own points are the points
    /// themselves: no instruction stands between (ADR 0006).
    fn set_point_with_roles(ch: &mut Chain, slots: [usize; 2], p: &PointPlan, roles: [&'static str; 2], attr: &'static str) {
        for j in 0..2 {
            match p.coord(j) {
                SourcePlan::Literal(v) => {
                    ch.from_attr(roles[j], attr);
                    ch.set_essential(slots[j], roles[j], v)
                }
                other => ch.set(slots[j], other),
            }
        }
    }

    fn point_slots_with_roles(ch: &mut Chain, p: &PointPlan, roles: [&'static str; 2], attr: &'static str) -> [u8; 2] {
        let mut out = [0u8; 2];
        for j in 0..2 {
            out[j] = match p.coord(j) {
                SourcePlan::Literal(v) => {
                    ch.from_attr(roles[j], attr);
                    ch.essential(roles[j], v)
                }
                other => ch.hidden(other),
            };
        }
        out
    }

    /// The points of a point-list attribute: bare references, parenthesized
    /// tuples (which may contain references), and references to array props
    /// (`$l.points`), which contribute one point per item.
    fn plan_point_list(&mut self, t: TemplateId, scope: ElemId, a: u32) -> Result<Vec<PointPlan>> {
        let d = self.c.dast;
        let (toks, text) = self.math_tokens(t, scope, d.attr_children(a))?;
        let mut out = Vec::new();
        let mut i = 0;
        while i < toks.len() {
            match &toks[i] {
                Token::Cell(p) => {
                    let p = *p as PlanId;
                    // An array prop expands to its items.
                    let items = self.array_items_of_plan(t, p)?;
                    match items {
                        Some(n) => out.extend((0..n).map(|k| PointPlan::Item(p, k))),
                        None => out.push(PointPlan::Ref(p)),
                    }
                    i += 1;
                }
                Token::LParen => {
                    let start = i;
                    let mut depth = 0i32;
                    loop {
                        match toks.get(i) {
                            Some(Token::LParen) => depth += 1,
                            Some(Token::RParen) => depth -= 1,
                            None => {
                                return Err(Error::BadMath { text, reason: "missing ')'".into() });
                            }
                            _ => {}
                        }
                        i += 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    let inner = crate::expr::unwrap_parens(&toks[start..i]).unwrap();
                    let parts = crate::expr::split_top(inner, &Token::Comma);
                    if parts.len() != 2 {
                        return Err(Error::BadMath { text, reason: format!("expected 2 coordinates, got {}", parts.len()) });
                    }
                    let x = Parser::parse(&parts[0], &mut self.c.arena).map_err(|reason| Error::BadMath { text: text.clone(), reason })?;
                    let y = Parser::parse(&parts[1], &mut self.c.arena).map_err(|reason| Error::BadMath { text: text.clone(), reason })?;
                    out.push(PointPlan::Tuple([self.plan_from_expr(x), self.plan_from_expr(y)]));
                }
                Token::Comma => i += 1,
                // A symbol (`through="A"`) is not a point here: NaN, as the
                // current core's warning case.
                Token::Ident(_) => {
                    out.push(PointPlan::Tuple([SourcePlan::Fixed(f64::NAN), SourcePlan::Fixed(f64::NAN)]));
                    i += 1;
                }
                other => {
                    return Err(Error::BadMath { text, reason: format!("unexpected {other:?} in a point list") });
                }
            }
        }
        Ok(out)
    }

    /// If plan `p` names an array prop, how many items it has (known at
    /// compile time from the referent element).
    fn array_items_of_plan(&self, t: TemplateId, p: PlanId) -> Result<Option<usize>> {
        let plan = &self.c.plans[p];
        let Some(prop) = &plan.prop else {
            return Ok(None);
        };
        // Find the element the path ends at, if it is in this template chain.
        let mut cur_t = t;
        for _ in 0..plan.hops {
            cur_t = self.c.templates[cur_t].parent.unwrap().0;
        }
        let mut e = None;
        for step in &plan.steps {
            match step {
                Step::Elem(x) => e = Some(*x),
                Step::Index(_) => return Ok(None),
            }
        }
        let Some(e) = e else { return Ok(None) };
        let el = &self.c.templates[cur_t].elems[e];
        let Some(items) = el.kind.array_prop(prop) else {
            return Ok(None);
        };
        let n = match el.kind {
            ComponentKind::Polygon => self.count_points_in_attr(el.node, "vertices")?,
            ComponentKind::Circle => self.count_points_in_attr(el.node, "through")?,
            _ => items.len(),
        };
        Ok(Some(n.min(items.len())))
    }

    /// Points in a point-list attribute, counted from its text alone.
    fn count_points_in_attr(&self, el: NodeId, name: &str) -> Result<usize> {
        let d = self.c.dast;
        let Some(a) = (el != NONE).then(|| d.attr(el, name)).flatten() else {
            return Ok(0);
        };
        let mut n = 0;
        let mut depth = 0i32;
        for &node in d.attr_children(a) {
            match d.kind(node) {
                NodeKind::Macro if depth == 0 => n += 1,
                NodeKind::Text => {
                    for c in d.str(node).chars() {
                        match c {
                            '(' => {
                                if depth == 0 {
                                    n += 1;
                                }
                                depth += 1;
                            }
                            ')' => depth -= 1,
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(n)
    }

    fn plan_circle(&mut self, t: TemplateId, e: ElemId, base: Option<NodeId>) -> Result<Chain> {
        let d = self.c.dast;
        let (el, scope) = {
            let x = &self.c.templates[t].elems[e];
            (x.node, x.name_scope)
        };
        let kind = ComponentKind::Circle;
        let center = match self.geo_attr(el, base, "center") {
            Some(a) => Some(match self.single_macro(a) {
                Some(m) => CenterPlan::Ref(self.plan_ref(t, scope, m)?),
                None => match self.plan_tuple(t, scope, d.attr_children(a)) {
                    Ok(xy) => CenterPlan::Tuple(xy),
                    // Not a point (`center="A"`): no center, as the current
                    // core's warning case.
                    Err(Error::BadMath { .. }) => CenterPlan::Tuple(vec![SourcePlan::Fixed(f64::NAN), SourcePlan::Fixed(f64::NAN)]),
                    Err(e) => return Err(e),
                },
            }),
            None => None,
        };
        let radius = match self.geo_attr(el, base, "radius") {
            Some(a) => Some(self.plan_scalar(t, scope, "radius", d.attr_children(a))?),
            None => None,
        };
        let through = match self.geo_attr(el, base, "through") {
            Some(a) => self.plan_point_list(t, scope, a)?,
            None => Vec::new(),
        };
        let n = through.len();
        let mut ch = Chain::new(kind.prop_defs().len());
        // Computed public props (diameter, area...) keep their defs.
        for (i, def) in kind.prop_defs().iter().enumerate() {
            if let PropFrom::Computed { op, args } = def.from {
                ch.set(i, SourcePlan::Computed(op, args.to_vec()));
            }
        }
        let (hc, hr) = (center.is_some(), radius.is_some());
        let center_slots = |ch: &mut Chain, c: &CenterPlan| -> [SourcePlan; 2] {
            match c {
                CenterPlan::Ref(p) => {
                    let _ = ch;
                    [SourcePlan::RefPart(*p, 0, 2), SourcePlan::RefPart(*p, 1, 2)]
                }
                CenterPlan::Tuple(v) => [v[0].clone(), v[1].clone()],
            }
        };
        // The radius shown is never negative; the prescribed or essential
        // radius behind it receives the clamped value (projection, ADR 0003).
        let clamped_radius = |ch: &mut Chain, r: &SourcePlan| -> SourcePlan {
            let pres = match r {
                SourcePlan::Literal(v) => {
                    ch.from_attr("r", "radius");
                    ch.essential("r", *v)
                }
                other => ch.hidden(other.clone()),
            };
            SourcePlan::Computed(OpSpec::Clamp { lo: 0.0, hi: f64::INFINITY }, vec![pres])
        };
        let essential_radius = |ch: &mut Chain| -> SourcePlan {
            let pres = ch.essential("r", 1.0);
            SourcePlan::Computed(OpSpec::Clamp { lo: 0.0, hi: f64::INFINITY }, vec![pres])
        };
        let nan = || SourcePlan::Fixed(f64::NAN);
        // Through points and the center-as-reference, for `$c.throughPoint1`
        // and `<point extend="$c.center">`.
        for (i, p) in through.iter().enumerate().take(3) {
            for j in 0..2 {
                match p.coord(j) {
                    SourcePlan::Literal(v) => {
                        ch.from_attr(POINT_ROLES[i][j], "through");
                        ch.set_essential(9 + 2 * i + j, POINT_ROLES[i][j], v)
                    }
                    other => ch.set(9 + 2 * i + j, other),
                }
            }
        }
        for i in through.len()..3 {
            ch.set(9 + 2 * i, nan());
            ch.set(10 + 2 * i, nan());
        }
        ch.set(15, SourcePlan::Fixed(n as f64));
        // The prescribed center lives on `centerX1`/`centerX2` (slots 7, 8)
        // and the cases read it from there; without one they alias the
        // derived center.
        match &center {
            Some(c) => {
                let [cx, cy] = center_slots(&mut ch, c);
                for (i, plan) in [cx, cy].into_iter().enumerate() {
                    match plan {
                        SourcePlan::Literal(v) => {
                            ch.from_attr(["cx", "cy"][i], "center");
                            ch.set_essential(7 + i, ["cx", "cy"][i], v)
                        }
                        other => ch.set(7 + i, other),
                    }
                }
            }
            None => {
                ch.set(7, SourcePlan::AliasOwn(0));
                ch.set(8, SourcePlan::AliasOwn(1));
            }
        }
        match (hc, hr, n) {
            // The current core warns and gives up on over-determined circles.
            (_, _, n) if n > 3 || (hc && hr && n >= 1) || (hc && n >= 2) || (hr && n >= 3) => {
                ch.set(0, nan());
                ch.set(1, nan());
                ch.set(2, nan());
            }
            (false, false, 0) => {
                ch.set_essential(0, "cx", 0.0);
                ch.set_essential(1, "cy", 0.0);
                let r = essential_radius(&mut ch);
                ch.set(2, r);
            }
            (true, false, 0) => {
                ch.set(0, SourcePlan::AliasOwn(7));
                ch.set(1, SourcePlan::AliasOwn(8));
                let r = essential_radius(&mut ch);
                ch.set(2, r);
            }
            (false, true, 0) => {
                ch.set_essential(0, "cx", 0.0);
                ch.set_essential(1, "cy", 0.0);
                let r = clamped_radius(&mut ch, radius.as_ref().unwrap());
                ch.set(2, r);
            }
            (true, true, 0) => {
                ch.set(0, SourcePlan::AliasOwn(7));
                ch.set(1, SourcePlan::AliasOwn(8));
                let r = clamped_radius(&mut ch, radius.as_ref().unwrap());
                ch.set(2, r);
            }
            (true, false, 1) => {
                ch.set(0, SourcePlan::Vec(VecOp::CircleCenterPoint, vec![7, 8, 9, 10]));
                ch.set(1, SourcePlan::VecOut(0, 1));
                ch.set(2, SourcePlan::VecOut(0, 2));
            }
            (false, has_r, 1) => {
                // The through point sits on top of the circle.
                let r = if has_r { clamped_radius(&mut ch, radius.as_ref().unwrap()) } else { essential_radius(&mut ch) };
                ch.set(2, r);
                ch.set(0, SourcePlan::AliasOwn(9));
                ch.set(1, SourcePlan::Computed(OpSpec::Sub, vec![10, 2]));
            }
            (false, true, 2) => {
                let r = clamped_radius(&mut ch, radius.as_ref().unwrap());
                ch.set(2, r);
                ch.set(0, SourcePlan::Vec(VecOp::CircleTwoPointsRadius, vec![9, 10, 11, 12, 2]));
                ch.set(1, SourcePlan::VecOut(0, 1));
            }
            (false, false, n) => {
                let args: Vec<u8> = (0..2 * n as u8).map(|k| 9 + k).collect();
                ch.set(0, SourcePlan::Vec(VecOp::CirclePoints { n: n as u8 }, args));
                ch.set(1, SourcePlan::VecOut(0, 1));
                ch.set(2, SourcePlan::VecOut(0, 2));
            }
            _ => unreachable!(),
        }
        Ok(ch)
    }

    /// Replace cell leaves that name a `<math>` element of this template
    /// with that math's own expression when it has free symbols.
    fn inline_symbolic_maths(&mut self, t: TemplateId, id: ExprId) -> ExprId {
        let e = self.c.arena.get(id).clone();
        match e {
            Expr::Cell(p) => {
                let plan = &self.c.plans[p as usize];
                let is_value = plan.prop.as_deref().is_none_or(|pr| pr == "value");
                let Some(target) = (is_value).then(|| self.plan_elem_target(t, p as PlanId)).flatten() else {
                    return id;
                };
                // The math may come later in the document and not be planned yet.
                if self.c.templates[t].elems[target].kind == ComponentKind::Math && matches!(self.c.templates[t].elems[target].body, Body::Plain) && self.plan_elem(t, target).is_err() {
                    return id;
                }
                match self.c.templates[t].elems[target].body {
                    Body::Math(inner) => {
                        let mut syms = Vec::new();
                        self.c.arena.symbols(inner, &mut syms);
                        if syms.is_empty() {
                            return id;
                        }
                        self.inline_symbolic_maths(t, inner)
                    }
                    _ => id,
                }
            }
            Expr::Num(_) | Expr::Sym(_) => id,
            Expr::Neg(a) => {
                let a = self.inline_symbolic_maths(t, a);
                self.c.arena.push(Expr::Neg(a))
            }
            Expr::Add(a, b) | Expr::Sub(a, b) | Expr::Mul(a, b) | Expr::Div(a, b) | Expr::Pow(a, b) => {
                let (na, nb) = (self.inline_symbolic_maths(t, a), self.inline_symbolic_maths(t, b));
                self.c.arena.push(match e {
                    Expr::Add(..) => Expr::Add(na, nb),
                    Expr::Sub(..) => Expr::Sub(na, nb),
                    Expr::Mul(..) => Expr::Mul(na, nb),
                    Expr::Div(..) => Expr::Div(na, nb),
                    _ => Expr::Pow(na, nb),
                })
            }
        }
    }

    /// Variable names of a line's equation (`variables="(s,t)"` or `"s t"`).
    fn line_variables(&self, el: NodeId, base: Option<NodeId>) -> (String, String) {
        match self.geo_attr(el, base, "variables").and_then(|a| self.attr_text(a)) {
            Some(text) => {
                let names: Vec<String> = text.split(|c: char| !c.is_alphanumeric()).filter(|s| !s.is_empty()).map(str::to_string).collect();
                if names.len() == 2 { (names[0].clone(), names[1].clone()) } else { ("x".into(), "y".into()) }
            }
            None => ("x".into(), "y".into()),
        }
    }

    fn plan_line(&mut self, t: TemplateId, e: ElemId, base: Option<NodeId>) -> Result<Chain> {
        let d = self.c.dast;
        let (el, scope) = {
            let x = &self.c.templates[t].elems[e];
            (x.node, x.name_scope)
        };
        let kind = ComponentKind::Line;
        let mut ch = Chain::new(kind.prop_defs().len());
        let through = match self.geo_attr(el, base, "through") {
            Some(a) => self.plan_point_list(t, scope, a)?,
            None => Vec::new(),
        };
        let slope = self.geo_attr(el, base, "slope");
        let parallel = self.geo_attr(el, base, "parallelTo");
        let perpendicular = self.geo_attr(el, base, "perpendicularTo");
        let equation_attr = self.geo_attr(el, base, "equation");
        let is_math_child = |me: &Self, n: NodeId| !me.is_blank(n) && d.kind(n) != NodeKind::Element;
        let own_children: Vec<NodeId> = d.children(el).iter().copied().filter(|&n| is_math_child(self, n)).collect();
        let base_children: Vec<NodeId> = base.map(|b| d.children(b).iter().copied().filter(|&n| is_math_child(self, n)).collect()).unwrap_or_default();
        let equation_nodes: Option<Vec<NodeId>> = if let Some(a) = equation_attr {
            Some(d.attr_children(a).to_vec())
        } else if !own_children.is_empty() {
            Some(own_children)
        } else if !base_children.is_empty() && through.is_empty() && slope.is_none() && parallel.is_none() && perpendicular.is_none() {
            Some(base_children)
        } else {
            None
        };
        if through.len() > 2 {
            return Err(Error::Unsupported("a line through more than two points".into()));
        }

        if let Some(nodes) = equation_nodes {
            // Equation mode: coefficients are the state, points follow.
            let (vx, vy) = self.line_variables(el, base);
            let (toks, text) = self.math_tokens(t, scope, &nodes)?;
            let sides = crate::expr::split_top(&toks, &Token::Eq);
            if sides.len() != 2 {
                return Err(Error::BadMath { text, reason: "a line equation needs one '='".into() });
            }
            let lhs = Parser::parse(&sides[0], &mut self.c.arena).map_err(|reason| Error::BadMath { text: text.clone(), reason })?;
            let rhs = Parser::parse(&sides[1], &mut self.c.arena).map_err(|reason| Error::BadMath { text: text.clone(), reason })?;
            let diff = self.c.arena.push(Expr::Sub(lhs, rhs));
            // A referenced <math> whose expression carries the variables
            // (`$m` standing for `2x`) is inlined, so it takes part in the
            // linear extraction instead of being an opaque cell.
            let diff = self.inline_symbolic_maths(t, diff);
            let coeffs = match crate::expr::linear_coeffs(&mut self.c.arena, diff, &vx, &vy) {
                // A symbolic or nonlinear equation is not a line here.
                None => [SourcePlan::Fixed(f64::NAN), SourcePlan::Fixed(f64::NAN), SourcePlan::Fixed(f64::NAN)],
                Some([a, b, c]) => [self.plan_from_expr(a), self.plan_from_expr(b), self.plan_from_expr(c)],
            };
            let [a, b, c] = coeffs;
            ch.set(7, a);
            ch.set(8, b);
            ch.set(9, c);
            ch.set(0, SourcePlan::Vec(VecOp::LinePointsFromCoeffs, vec![7, 8, 9]));
            ch.set(1, SourcePlan::VecOut(0, 1));
            ch.set(2, SourcePlan::VecOut(0, 2));
            ch.set(3, SourcePlan::VecOut(0, 3));
            let q = ch.hidden(SourcePlan::Computed(OpSpec::Div, vec![7, 8]));
            ch.set(4, SourcePlan::Computed(OpSpec::Negate, vec![q]));
            let xi = ch.hidden(SourcePlan::Computed(OpSpec::Div, vec![9, 7]));
            ch.set(5, SourcePlan::Computed(OpSpec::Negate, vec![xi]));
            let yi = ch.hidden(SourcePlan::Computed(OpSpec::Div, vec![9, 8]));
            ch.set(6, SourcePlan::Computed(OpSpec::Negate, vec![yi]));
            ch.set(10, SourcePlan::Fixed(0.0));
            return Ok(ch);
        }

        let direction_mode = through.len() < 2 && (slope.is_some() || parallel.is_some() || perpendicular.is_some());
        // First point: a through point or the essential default, which the
        // current core takes as (1, 0) for a two-point line and (0, 0) when a
        // slope or direction gives the second point.
        // The essential defaults carry the current core's roles: `ess1` is
        // (1, 0) and `ess2` is (0, 0); a direction-based line's first point
        // is `ess2`.
        let mut slope_slot: Option<u8> = None;
        let mut direction_slots: Option<[u8; 2]> = None;
        if direction_mode {
            match through.first() {
                Some(p) => {
                    for j in 0..2 {
                        match p.coord(j) {
                            SourcePlan::Literal(v) => {
                                ch.from_attr(POINT_ROLES[0][j], "through");
                                ch.set_essential(j, POINT_ROLES[0][j], v)
                            }
                            other => ch.set(j, other),
                        }
                    }
                }
                None => {
                    ch.set_essential(0, "ess2x", 0.0);
                    ch.set_essential(1, "ess2y", 0.0);
                }
            }
            let dist = ch.essential("dist", 1.0);
            if let Some(a) = slope {
                let m = self.plan_scalar(t, scope, "slope", d.attr_children(a))?;
                let m = ch.hidden(m);
                slope_slot = Some(m);
                ch.set(2, SourcePlan::Vec(VecOp::PolarSlope, vec![0, 1, m, dist]));
            } else {
                let (a, perp) = match (parallel, perpendicular) {
                    (Some(a), _) => (a, false),
                    (None, Some(a)) => (a, true),
                    _ => unreachable!(),
                };
                let [ux, uy] = self.plan_direction(t, scope, a, &mut ch)?;
                direction_slots = Some(if perp { [uy, ux] } else { [ux, uy] });
                ch.set(2, SourcePlan::Vec(VecOp::PolarDirection { perpendicular: perp }, vec![0, 1, ux, uy, dist]));
            }
            ch.set(3, SourcePlan::VecOut(2, 1));
            ch.set(10, SourcePlan::Fixed(1.0));
        } else {
            // The line's points are the through points (or essential
            // defaults) themselves; a whole-line drag is a point group.
            let pt = |ch: &mut Chain, slots: [usize; 2], p: Option<&PointPlan>, k: usize, roles: [&'static str; 2], default: [f64; 2]| match p {
                Some(p) => Self::set_point_with_roles(ch, slots, p, POINT_ROLES[k], "through"),
                None => {
                    ch.set_essential(slots[0], roles[0], default[0]);
                    ch.set_essential(slots[1], roles[1], default[1]);
                }
            };
            pt(&mut ch, [0, 1], through.first(), 0, ["ess1x", "ess1y"], [1.0, 0.0]);
            pt(&mut ch, [2, 3], through.get(1), 1, ["ess2x", "ess2y"], [0.0, 0.0]);
            ch.set(10, SourcePlan::Fixed(0.0));
        }
        // slope, intercepts and coefficients from the two points:
        // a = y2 - y1, b = x1 - x2, c = -(a x1 + b y1). The slope is the
        // points' ratio even for a slope-based line, as in the current core
        // (NaN when the points coincide).
        let dy = ch.hidden(SourcePlan::Computed(OpSpec::Sub, vec![3, 1]));
        let dx = ch.hidden(SourcePlan::Computed(OpSpec::Sub, vec![2, 0]));
        let _ = (slope_slot, direction_slots);
        ch.set(4, SourcePlan::Computed(OpSpec::Div, vec![dy, dx]));
        let q = ch.hidden(SourcePlan::Computed(OpSpec::Div, vec![1, 4]));
        ch.set(5, SourcePlan::Computed(OpSpec::Sub, vec![0, q]));
        let mx = ch.hidden(SourcePlan::Computed(OpSpec::Mul, vec![4, 0]));
        ch.set(6, SourcePlan::Computed(OpSpec::Sub, vec![1, mx]));
        ch.set(7, SourcePlan::AliasOwn(dy));
        ch.set(8, SourcePlan::Computed(OpSpec::Sub, vec![0, 2]));
        let ax = ch.hidden(SourcePlan::Computed(OpSpec::Mul, vec![7, 0]));
        let by = ch.hidden(SourcePlan::Computed(OpSpec::Mul, vec![8, 1]));
        let sum = ch.hidden(SourcePlan::Computed(OpSpec::Add, vec![ax, by]));
        ch.set(9, SourcePlan::Computed(OpSpec::Negate, vec![sum]));
        Ok(ch)
    }

    /// Two hidden slots holding a direction: a point or tuple's coordinates,
    /// or another line's `point2 - point1`.
    fn plan_direction(&mut self, t: TemplateId, scope: ElemId, a: u32, ch: &mut Chain) -> Result<[u8; 2]> {
        let d = self.c.dast;
        match self.single_macro(a) {
            Some(m) => {
                let p = self.plan_ref(t, scope, m)?;
                let target_kind = self.plan_elem_target(t, p).map(|e| self.c.templates[t].elems[e].kind);
                match target_kind {
                    Some(ComponentKind::Line | ComponentKind::LineSegment) => {
                        let x1 = self.plan_with_prop(p, "x1");
                        let y1 = self.plan_with_prop(p, "y1");
                        let x2 = self.plan_with_prop(p, "x2");
                        let y2 = self.plan_with_prop(p, "y2");
                        let ux = ch.hidden(SourcePlan::Op(OpSpec::Sub, vec![x2, x1]));
                        let uy = ch.hidden(SourcePlan::Op(OpSpec::Sub, vec![y2, y1]));
                        Ok([ux, uy])
                    }
                    _ => Ok(Self::point_slots(ch, &PointPlan::Ref(p))),
                }
            }
            None => {
                let xy = self.plan_tuple(t, scope, d.attr_children(a))?;
                Ok([ch.hidden(xy[0].clone()), ch.hidden(xy[1].clone())])
            }
        }
    }

    fn plan_segment(&mut self, t: TemplateId, e: ElemId, base: Option<NodeId>) -> Result<Chain> {
        let (el, scope) = {
            let x = &self.c.templates[t].elems[e];
            (x.node, x.name_scope)
        };
        let mut ch = Chain::new(ComponentKind::LineSegment.prop_defs().len());
        let ends = match self.geo_attr(el, base, "endpoints") {
            Some(a) => self.plan_point_list(t, scope, a)?,
            None => Vec::new(),
        };
        let pt = |ch: &mut Chain, slots: [usize; 2], p: Option<&PointPlan>, k: usize, roles: [&'static str; 2], default: [f64; 2]| match p {
            Some(p) => Self::set_point_with_roles(ch, slots, p, POINT_ROLES[k], "endpoints"),
            None => {
                ch.set_essential(slots[0], roles[0], default[0]);
                ch.set_essential(slots[1], roles[1], default[1]);
            }
        };
        pt(&mut ch, [0, 1], ends.first(), 0, ["ess1x", "ess1y"], [1.0, 0.0]);
        pt(&mut ch, [2, 3], ends.get(1), 1, ["ess2x", "ess2y"], [0.0, 0.0]);
        Ok(ch)
    }

    fn plan_polygon(&mut self, t: TemplateId, e: ElemId, base: Option<NodeId>) -> Result<Chain> {
        let d = self.c.dast;
        let (el, scope) = {
            let x = &self.c.templates[t].elems[e];
            (x.node, x.name_scope)
        };
        let kind = ComponentKind::Polygon;
        let mut ch = Chain::new(kind.prop_defs().len());
        let vertices = match self.geo_attr(el, base, "vertices") {
            Some(a) => self.plan_point_list(t, scope, a)?,
            None => Vec::new(),
        };
        let n = vertices.len();
        if n > crate::components::MAX_VERTICES {
            return Err(Error::Unsupported(format!("a polygon with more than {} vertices", crate::components::MAX_VERTICES)));
        }
        let flag = |me: &Self, name: &str, default: bool| -> bool {
            match me.geo_attr(el, base, name) {
                None => default,
                Some(a) => {
                    let text = me.attr_text(a).unwrap_or_default();
                    let text = text.trim();
                    text.is_empty() || text.eq_ignore_ascii_case("true")
                }
            }
        };
        let rigid = flag(self, "rigid", false);
        let similar = flag(self, "preserveSimilarity", false);
        let pivot_point = match self.geo_attr(el, base, "rotationCenter") {
            Some(a) if flag(self, "rotateAround", false) || self.geo_attr(el, base, "rotateAround").and_then(|r| self.attr_text(r)).is_some_and(|r| r.trim() == "point") => match self.single_macro(a) {
                Some(m) => Some(PointPlan::Ref(self.plan_ref(t, scope, m)?)),
                None => {
                    let xy = self.plan_tuple(t, scope, d.attr_children(a))?;
                    Some(PointPlan::Tuple([xy[0].clone(), xy[1].clone()]))
                }
            },
            _ => None,
        };
        let rigid_opts = if rigid || similar {
            let rotate_around = self.geo_attr(el, base, "rotateAround").and_then(|a| self.attr_text(a)).unwrap_or_default();
            let pivot = match rotate_around.trim() {
                "vertex" => {
                    let k = self.geo_attr(el, base, "rotationVertex").and_then(|a| self.attr_text(a)).and_then(|s| s.trim().parse::<usize>().ok()).unwrap_or(1);
                    Pivot::Vertex(k.saturating_sub(1) as u8)
                }
                "point" if pivot_point.is_some() => Pivot::Input,
                _ => Pivot::Centroid,
            };
            let min_shrink = self.geo_attr(el, base, "minShrink").and_then(|a| self.attr_text(a)).and_then(|s| s.trim().parse::<f64>().ok()).unwrap_or(0.1);
            Some(RigidOpts {
                // `rigid` forbids dilation; `preserveSimilarity` allows it
                // unless `allowDilation` says otherwise.
                dilate: !rigid && flag(self, "allowDilation", true),
                rotate: flag(self, "allowRotation", true),
                translate: flag(self, "allowTranslation", true),
                min_shrink,
                pivot,
            })
        } else {
            None
        };
        ch.set(0, SourcePlan::Fixed(n as f64));
        match rigid_opts {
            // Rigid: the document declares the coupling, so one instruction
            // owns every vertex.
            Some(opts) if n > 0 => {
                let mut args = Vec::with_capacity(2 * n + 2);
                for (k, v) in vertices.iter().enumerate() {
                    let [x, y] = Self::point_slots_with_roles(&mut ch, v, POINT_ROLES[k], "vertices");
                    args.push(x);
                    args.push(y);
                }
                if let (Pivot::Input, Some(pp)) = (opts.pivot, &pivot_point) {
                    let [x, y] = Self::point_slots(&mut ch, pp);
                    args.push(x);
                    args.push(y);
                }
                ch.set(1, SourcePlan::Vec(VecOp::Shape { n: n as u8, opts }, args));
                for k in 1..2 * n {
                    ch.set(1 + k, SourcePlan::VecOut(1, k as u8));
                }
            }
            // Free: the vertices are the points; a vertex may be defined
            // from its siblings without a cycle.
            _ => {
                for (k, v) in vertices.iter().enumerate() {
                    Self::set_point_with_roles(&mut ch, [1 + 2 * k, 2 + 2 * k], v, POINT_ROLES[k], "vertices");
                }
            }
        }
        for k in 2 * n..2 * crate::components::MAX_VERTICES {
            ch.set(1 + k, SourcePlan::Fixed(f64::NAN));
        }
        Ok(ch)
    }

    // ---- reference plans -----------------------------------------------------

    /// Find a name as the current core's resolver does: from the
    /// referencing element `from` (or `ROOT_SCOPE`) walk up the ancestors;
    /// at each, the ancestor's own name wins, then a unique descendant with
    /// the name; several descendants are an ambiguity. Then continue in the
    /// enclosing template from the repeat element. Returns (hops, element).
    fn lookup(&self, mut t: TemplateId, mut from: ElemId, name: &str) -> Result<Option<(u32, ElemId)>> {
        let name = name.trim();
        let mut hops = 0;
        loop {
            let tpl = &self.c.templates[t];
            let mut a = from;
            loop {
                if a != ROOT_SCOPE && tpl.elems[a].name != NONE && self.c.dast.strings.get(tpl.elems[a].name).trim() == name {
                    return Ok(Some((hops, a)));
                }
                match tpl.names.get(&(a, name.to_string())).map(Vec::as_slice) {
                    Some([e]) => return Ok(Some((hops, *e))),
                    Some([_, _, ..]) => return Err(Error::AmbiguousName(name.to_string())),
                    _ => {}
                }
                if a == ROOT_SCOPE {
                    break;
                }
                a = tpl.elems[a].name_scope;
            }
            let Some((pt, pe)) = tpl.parent else {
                return Ok(None);
            };
            from = pe;
            t = pt;
            hops += 1;
        }
    }

    /// A unique descendant of `e` with `name`, for a dotted path.
    fn child_named(&self, t: TemplateId, e: ElemId, name: &str) -> Result<Option<ElemId>> {
        match self.c.templates[t].names.get(&(e, name.trim().to_string())).map(Vec::as_slice) {
            Some([c]) => Ok(Some(*c)),
            Some([_, _, ..]) => Err(Error::AmbiguousName(name.trim().to_string())),
            _ => Ok(None),
        }
    }

    fn plan_ref(&mut self, t: TemplateId, scope: ElemId, m: NodeId) -> Result<PlanId> {
        let d = self.c.dast;
        let display = d.macro_display(m);
        let names = d.macro_path(m);
        let parts: Vec<_> = d.macro_parts(m).collect();
        let first = d.strings.get(names[0]);
        let (hops, e0) = self.lookup(t, scope, first)?.ok_or_else(|| Error::UnknownName(first.into()))?;
        let mut cur_t = t;
        for _ in 0..hops {
            cur_t = self.c.templates[cur_t].parent.unwrap().0;
        }
        let mut steps = vec![Step::Elem(e0)];
        // The element the path stands at, or None right after an index step
        // into a repeat (inside an iteration, before a name picks an element).
        let mut cur_elem = Some(e0);
        let mut prop = None;
        for (i, &part) in parts.iter().enumerate() {
            if i > 0 {
                let name = d.strings.get(names[i]);
                match cur_elem {
                    None => {
                        let e = self.child_named(cur_t, ROOT_SCOPE, name)?.ok_or_else(|| Error::UnknownName(display.clone()))?;
                        steps.push(Step::Elem(e));
                        cur_elem = Some(e);
                    }
                    // A descendant of the component: `$g.p`.
                    Some(e) if self.child_named(cur_t, e, name)?.is_some() => {
                        let child = self.child_named(cur_t, e, name)?.unwrap();
                        steps.push(Step::Elem(child));
                        cur_elem = Some(child);
                    }
                    Some(e) => {
                        let kind = self.c.templates[cur_t].elems[e].kind;
                        let kind = match self.c.templates[cur_t].elems[e].body {
                            // After `$c[k]` the component is a collected copy.
                            Body::Collect { kind: ck, .. } if steps.len() > 1 => ck,
                            _ => kind,
                        };
                        // `$l.points[1]`, `$l.points[1][2]`, `$l.points[2].y`:
                        // items of an array prop, by literal index.
                        if let Some(items) = kind.array_prop(name) {
                            let idx: Vec<i64> = d.part_indices(part).map(|expr| self.literal_index(expr, &display)).collect::<Result<_>>()?;
                            let (Some(&k), rest) = (idx.first(), &idx[1.min(idx.len())..]) else {
                                if i + 1 != parts.len() {
                                    return Err(Error::PathTooDeep(display));
                                }
                                prop = Some(name.to_string());
                                break;
                            };
                            if k < 1 || k as usize > items.len() {
                                return Err(Error::BadIndex(display));
                            }
                            let item = items[k as usize - 1];
                            let coord = match (rest.first(), parts.get(i + 1)) {
                                (Some(&j), None) => Some(j),
                                (None, Some(_)) if i + 2 == parts.len() => Some(match d.strings.get(names[i + 1]).trim() {
                                    "x" | "1" => 1,
                                    "y" | "2" => 2,
                                    _ => return Err(Error::PathTooDeep(display)),
                                }),
                                (None, None) => None,
                                _ => return Err(Error::PathTooDeep(display)),
                            };
                            prop = Some(match coord {
                                Some(1) => item[0].to_string(),
                                Some(2) => item[1].to_string(),
                                Some(_) => return Err(Error::BadIndex(display)),
                                None => kind.array_item_prop(name, k as usize).ok_or_else(|| Error::PathTooDeep(display.clone()))?,
                            });
                            break;
                        }
                        // `$l.point1[2]`: a coordinate of a point-valued prop.
                        if let (Some(parts_of), Some(expr)) = (kind.virtual_prop(name), d.part_indices(part).next()) {
                            let k = self.literal_index(expr, &display)?;
                            if !(1..=2).contains(&k) || d.part_indices(part).nth(1).is_some() || i + 1 != parts.len() {
                                return Err(Error::PathTooDeep(display));
                            }
                            prop = Some(parts_of[k as usize - 1].to_string());
                            break;
                        }
                        if d.part_indices(part).next().is_some() {
                            return Err(Error::PathTooDeep(display));
                        }
                        // A prop name, or a coordinate of a point-valued prop
                        // (`$c.center.y`), which must end the path.
                        if i + 1 != parts.len() {
                            let parts_of = kind.virtual_prop(name).ok_or_else(|| Error::PathTooDeep(display.clone()))?;
                            let coord = d.strings.get(names[i + 1]).trim();
                            let j = match coord {
                                "x" | "1" => 0,
                                "y" | "2" => 1,
                                _ => return Err(Error::PathTooDeep(display)),
                            };
                            if i + 2 != parts.len() {
                                return Err(Error::PathTooDeep(display));
                            }
                            prop = Some(parts_of[j].to_string());
                            break;
                        }
                        if kind.prop_index(name).is_none() && kind.virtual_prop(name).is_none() && kind.array_prop(name).is_none() {
                            return Err(Error::UnknownProp { name: self.elem_label(cur_t, e), prop: name.into() });
                        }
                        prop = Some(name.to_string());
                        break;
                    }
                }
            }
            for expr in d.part_indices(part) {
                let Some(e) = cur_elem else {
                    return Err(Error::NotIndexable(display));
                };
                // `$p[2]`: a coordinate of a point.
                let ek = self.c.templates[cur_t].elems[e].kind;
                if let Some(parts_of) = ek.default_prop().and_then(|dp| ek.virtual_prop(dp)) {
                    if prop.is_some() || i + 1 != parts.len() {
                        return Err(Error::PathTooDeep(display));
                    }
                    let k = self.literal_index(expr, &display)?;
                    if !(1..=parts_of.len() as i64).contains(&k) {
                        return Err(Error::BadIndex(display));
                    }
                    prop = Some(parts_of[k as usize - 1].to_string());
                    continue;
                }
                match self.c.templates[cur_t].elems[e].body {
                    Body::Repeat { template } => {
                        let ip = self.plan_index(t, expr, &display)?;
                        steps.push(Step::Index(ip));
                        cur_t = template;
                        cur_elem = None;
                    }
                    Body::Collect { .. } => {
                        let ip = self.plan_index(t, expr, &display)?;
                        steps.push(Step::Index(ip));
                        cur_elem = Some(e);
                    }
                    _ if self.c.templates[cur_t].elems[e].kind == ComponentKind::PointList => {
                        let ip = self.plan_index(t, expr, &display)?;
                        steps.push(Step::Index(ip));
                        cur_elem = Some(e);
                    }
                    _ => return Err(Error::NotIndexable(display)),
                }
            }
        }
        self.c.plans.push(RefPlan { hops, steps, prop, display });
        Ok(self.c.plans.len() - 1)
    }

    /// A literal integer index (array props are static, so `[$n]` is not
    /// supported on them).
    fn literal_index(&self, expr: &[NodeId], display: &str) -> Result<i64> {
        let d = self.c.dast;
        let mut text = String::new();
        for &n in expr {
            match d.kind(n) {
                NodeKind::Text => text.push_str(d.str(n)),
                NodeKind::Macro => return Err(Error::DynamicIndex(display.to_string())),
                _ => {}
            }
        }
        text.trim().parse::<i64>().map_err(|_| Error::BadIndex(display.to_string()))
    }

    /// An index expression: a sum of literal integers and iteration indices.
    fn plan_index(&self, t: TemplateId, expr: &[NodeId], display: &str) -> Result<IndexPlan> {
        let d = self.c.dast;
        let mut terms = Vec::new();
        for &n in expr {
            match d.kind(n) {
                NodeKind::Text => {
                    let s: String = d.str(n).chars().filter(|c| !c.is_whitespace()).collect();
                    if s.is_empty() {
                        continue;
                    }
                    terms.push(IndexTerm::Const(s.trim_start_matches('+').parse::<i64>().map_err(|_| Error::BadIndex(display.to_string()))?));
                }
                NodeKind::Macro => {
                    let path = d.macro_path(n);
                    if path.len() != 1 || d.macro_has_index(n) {
                        return Err(Error::DynamicIndex(display.to_string()));
                    }
                    let name = d.strings.get(path[0]);
                    let (hops, e) = self.lookup(t, ROOT_SCOPE, name)?.ok_or_else(|| Error::UnknownName(name.into()))?;
                    let mut tt = t;
                    for _ in 0..hops {
                        tt = self.c.templates[tt].parent.unwrap().0;
                    }
                    let el = &self.c.templates[tt].elems[e];
                    match (el.kind, el.props.first()) {
                        (ComponentKind::Number, Some(SourcePlan::IterIndex)) => terms.push(IndexTerm::Iter(hops)),
                        _ => return Err(Error::DynamicIndex(display.to_string())),
                    }
                }
                _ => {}
            }
        }
        Ok(IndexPlan { terms })
    }

    fn elem_label(&self, t: TemplateId, e: ElemId) -> String {
        let el = &self.c.templates[t].elems[e];
        if el.name != NONE { self.c.dast.strings.get(el.name).trim().to_string() } else { format!("<{}>", el.kind.tag()) }
    }

    fn attr_text(&self, a: u32) -> Option<String> {
        let d = self.c.dast;
        let mut s = String::new();
        for &n in d.attr_children(a) {
            match d.kind(n) {
                NodeKind::Text => s.push_str(d.str(n)),
                NodeKind::Macro => return None,
                _ => {}
            }
        }
        Some(s)
    }

    fn single_macro(&self, a: u32) -> Option<NodeId> {
        let d = self.c.dast;
        let mut found = None;
        for &n in d.attr_children(a) {
            match d.kind(n) {
                NodeKind::Macro if found.is_none() => found = Some(n),
                NodeKind::Macro => return None,
                NodeKind::Text if d.str(n).trim().is_empty() => {}
                NodeKind::Text => return None,
                _ => {}
            }
        }
        found
    }

    fn is_blank(&self, n: NodeId) -> bool {
        match self.c.dast.kind(n) {
            NodeKind::Text => self.c.dast.str(n).trim().is_empty(),
            NodeKind::Other => true,
            _ => false,
        }
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
    scopes: Vec<(ScopeId, NodeId, u32)>,
    scope_index: HashMap<(ScopeId, NodeId, u32), ScopeId>,
    counts: HashMap<(ScopeId, NodeId), u32>,
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
        Prior { scopes: doc.structure.scopes.clone(), scope_index: doc.structure.scope_index.clone(), counts, values }
    }

    /// Non-destructive variant for callers that keep the document.
    pub fn from_document(doc: &Document) -> Prior {
        let mut copy = doc.clone();
        Prior::take_from(&mut copy)
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
    OpVec(VecOp, u32, u8),
    /// Output `k` of the vector instruction headed at `head`.
    OutputOf(SlotId, u8),
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
    arena: Arena,
    extra: Vec<CellIdx>,
    /// Human-readable owner of a cell, e.g. "p1.x". Computed lazily because
    /// a cycle error is the only consumer.
    cell_label: Box<dyn Fn(CellIdx) -> String>,
}

impl Unscheduled {
    pub fn schedule(self, dast: std::sync::Arc<Dast>) -> Result<Document> {
        let n = self.cells.len();
        let program = Program::schedule(self.instrs, n, self.arena, self.extra).map_err(|cell| Error::Cycle((self.cell_label)(cell)))?;
        Ok(Document::new(self.cells, self.n_essential, self.n_fixed, program, self.comps, self.strings, self.root, self.structure, dast))
    }
}

pub fn build(dast: &Dast, prior: &Prior) -> Result<Unscheduled> {
    let profile = std::env::var_os("CELLS_BUILD_PROFILE").is_some();
    let clock = web_time::Instant::now();
    let lap = |what: &str| {
        if profile {
            eprintln!("  build/{what}: {:.2?}", clock.elapsed());
        }
    };
    let compiled = Compiler::compile(dast)?;
    lap("compile");
    let mut b = Builder::new(&compiled, prior);
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
pub fn build_once(dast: &Dast) -> Result<Unscheduled> {
    build(dast, &Prior::default())
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
    missing: Option<SlotId>,
    arena: Arena,
    root: CompIdx,
}

impl<'c, 'a> Builder<'c, 'a> {
    fn new(c: &'c Compiled<'a>, prior: &'c Prior) -> Self {
        // Size the columns from the previous build when there was one.
        let guess = prior.values.iter().map(|v| v.len()).sum::<usize>().max(c.templates.iter().map(|t| t.elems.len()).sum::<usize>() * 2);
        Builder {
            c,
            prior,
            comps: Components {
                kind: Vec::with_capacity(guess),
                name: Vec::with_capacity(guess),
                parent: Vec::with_capacity(guess),
                prop_base: Vec::new(),
                prop_cells: Vec::new(),
                child_start: Vec::with_capacity(guess),
                child_count: Vec::with_capacity(guess),
                child_list: Vec::with_capacity(guess),
                node: Vec::with_capacity(guess),
                scope: Vec::with_capacity(guess),
            },
            slot_base: Vec::with_capacity(guess),
            sources: Vec::with_capacity(guess * 2),
            slot_comp: Vec::with_capacity(guess * 2),
            op_inputs: Vec::with_capacity(guess),
            scopes: if prior.scopes.is_empty() { vec![(NONE, NONE, 0)] } else { prior.scopes.clone() },
            scope_index: prior.scope_index.clone(),
            scope_comps: Vec::new(),
            comp_instance: Vec::with_capacity(guess),
            instances: Vec::with_capacity(guess),
            comp_repeat: Vec::with_capacity(guess),
            repeats: Vec::new(),
            counts_used: Vec::new(),
            pending: Vec::new(),
            collects: Vec::new(),
            pointlists: Vec::new(),
            collected: HashMap::new(),
            missing: None,
            arena: Arena::default(),
            root: 0,
        }
    }

    // ---- components and slots ------------------------------------------------

    /// `n_slots` is the public prop count plus any hidden slots the
    /// element's plan added.
    fn new_component(&mut self, kind: ComponentKind, name: StrId, parent: CompIdx, node: NodeId, scope: ScopeId, n_slots: usize) -> CompIdx {
        let idx = self.comps.len() as CompIdx;
        self.comps.kind.push(kind);
        self.comps.name.push(name);
        self.comps.parent.push(parent);
        self.comps.child_start.push(0);
        self.comps.child_count.push(0);
        self.comps.node.push(node);
        self.comps.scope.push(scope);
        self.comp_instance.push(NONE);
        self.comp_repeat.push(NONE);
        self.slot_base.push(NONE);
        self.allocate_slots(idx, n_slots);
        idx
    }

    fn allocate_slots(&mut self, comp: CompIdx, n: usize) {
        self.slot_base[comp as usize] = self.sources.len() as u32;
        self.sources.extend(std::iter::repeat_n(Source::Unset, n));
        self.slot_comp.extend(std::iter::repeat_n(comp, n));
    }

    #[inline]
    fn slot(&self, comp: CompIdx, prop_index: usize) -> SlotId {
        self.slot_base[comp as usize] + prop_index as u32
    }

    fn set_children(&mut self, comp: CompIdx, kids: &[u32]) {
        self.comps.child_start[comp as usize] = self.comps.child_list.len() as u32;
        self.comps.child_count[comp as usize] = kids.len() as u32;
        self.comps.child_list.extend_from_slice(kids);
    }

    /// A slot that belongs to no component: a lowered math subexpression, a
    /// literal inside one, the missing-referent cell.
    fn anon_slot(&mut self, source: Source) -> SlotId {
        let s = self.sources.len() as SlotId;
        self.sources.push(source);
        self.slot_comp.push(NONE);
        s
    }

    fn missing_slot(&mut self) -> SlotId {
        if let Some(s) = self.missing {
            return s;
        }
        let s = self.anon_slot(Source::Fixed(f64::NAN));
        self.missing = Some(s);
        s
    }

    fn comp_label(&self, comp: CompIdx) -> String {
        let name = self.comps.name[comp as usize];
        if name != NONE { self.c.dast.strings.get(name).trim().to_string() } else { format!("<{}>#{}", self.comps.kind[comp as usize].tag(), comp) }
    }

    fn slot_label(&self, slot: SlotId) -> String {
        let comp = self.slot_comp[slot as usize];
        if comp == NONE {
            return format!("(anonymous slot {slot})");
        }
        let pi = (slot - self.slot_base[comp as usize]) as usize;
        match self.comps.kind[comp as usize].prop_defs().get(pi) {
            Some(def) => format!("{}.{}", self.comp_label(comp), def.name),
            None => format!("{}.(hidden slot {pi})", self.comp_label(comp)),
        }
    }

    // ---- scopes --------------------------------------------------------------

    /// The stable id of iteration `k` of repeat element `node` under
    /// `parent`, created on first use.
    fn scope_for(&mut self, parent: ScopeId, node: NodeId, k: u32) -> ScopeId {
        if let Some(&s) = self.scope_index.get(&(parent, node, k)) {
            return s;
        }
        self.scopes.push((parent, node, k));
        let s = (self.scopes.len() - 1) as ScopeId;
        self.scope_index.insert((parent, node, k), s);
        s
    }

    fn enter_scope(&mut self, scope: ScopeId, template: TemplateId) {
        let s = scope as usize;
        if self.scope_comps.len() <= s {
            self.scope_comps.resize(s + 1, Vec::new());
        }
        self.scope_comps[s] = vec![NONE; self.c.templates[template].elems.len()];
    }

    // ---- expansion -----------------------------------------------------------

    fn expand_all(&mut self) -> Result<()> {
        self.enter_scope(0, 0);
        let kids = self.expand(0, 0, NONE)?;
        self.root = kids.iter().copied().find(|&k| k & TEXT_BIT == 0).expect("document root");
        Ok(())
    }

    /// Instantiate template `t` in `scope`; returns the child entries of the
    /// template's own children (the repeat body, or the document).
    fn expand(&mut self, t: TemplateId, scope: ScopeId, parent: CompIdx) -> Result<Vec<u32>> {
        let n_elems = self.c.templates[t].elems.len();
        // Create every element's component first so children can refer to them.
        for e in 0..n_elems {
            let el = &self.c.templates[t].elems[e];
            let n_slots = el.props.len().max(el.kind.prop_defs().len());
            let comp = self.new_component(el.kind, el.name, parent, el.node, scope, n_slots);
            self.scope_comps[scope as usize][e] = comp;
            self.comp_instance[comp as usize] = self.instances.len() as u32;
            self.instances.push(Instance { scope, template: t, elem: e, comp });
        }
        // Then child lists, expanding repeats as they come.
        for e in 0..n_elems {
            let comp = self.scope_comps[scope as usize][e];
            let kids = match self.c.templates[t].elems[e].body {
                Body::Repeat { template } => {
                    let node = self.c.templates[t].elems[e].node;
                    let n = self.prior.counts.get(&(scope, node)).copied().unwrap_or(0);
                    let mut kids = Vec::new();
                    let mut iter_scopes = Vec::with_capacity(n as usize);
                    for k in 1..=n {
                        let s = self.scope_for(scope, node, k);
                        iter_scopes.push(s);
                        self.enter_scope(s, template);
                        let mut these = self.expand(template, s, comp)?;
                        kids.append(&mut these);
                    }
                    self.comp_repeat[comp as usize] = self.repeats.len() as u32;
                    self.repeats.push(Repeat { comp, node, scope, iter_scopes, n });
                    self.counts_used.push(n);
                    kids
                }
                Body::Collect { .. } => {
                    self.collects.push(comp);
                    Vec::new()
                }
                Body::PointList { .. } => {
                    self.pointlists.push(comp);
                    Vec::new()
                }
                _ if self.c.templates[t].elems[e].kind == ComponentKind::PointList => {
                    // A copy of a point list: children copied once the
                    // original's exist.
                    self.pointlists.push(comp);
                    Vec::new()
                }
                _ => self.child_entries(t, e, scope, comp),
            };
            self.set_children(comp, &kids);
            for &k in &kids {
                if k & TEXT_BIT == 0 {
                    self.comps.parent[k as usize] = comp;
                }
            }
        }
        // The template's own children (the repeat body); their parent is the repeat.
        let mut out = Vec::with_capacity(self.c.templates[t].children.len());
        for i in 0..self.c.templates[t].children.len() {
            let ch = &self.c.templates[t].children[i];
            out.push(self.child_entry(ch, scope, parent));
        }
        Ok(out)
    }

    fn child_entries(&mut self, t: TemplateId, e: ElemId, scope: ScopeId, parent: CompIdx) -> Vec<u32> {
        let n = self.c.templates[t].elems[e].children.len();
        let mut kids = Vec::with_capacity(n);
        for i in 0..n {
            let ch = &self.c.templates[t].elems[e].children[i];
            kids.push(self.child_entry(ch, scope, parent));
        }
        kids
    }

    fn child_entry(&mut self, ch: &Child, scope: ScopeId, parent: CompIdx) -> u32 {
        match *ch {
            Child::Elem(e) => self.scope_comps[scope as usize][e],
            Child::Text(s) => TEXT_BIT | s,
            Child::Macro(plan, has_index) => {
                // Kind is unknown until the reference resolves; Document stands in.
                let idx = self.new_component(ComponentKind::Document, NONE, parent, NONE, scope, 0);
                self.pending.push((idx, plan, scope, has_index));
                idx
            }
        }
    }

    // ---- resolution ----------------------------------------------------------

    /// Placeholders without an index first (a collect may gather copies),
    /// then collects in document order, then indexed placeholders (which may
    /// name a collect's items), then every prop source.
    fn resolve_all(&mut self) -> Result<()> {
        let pending = std::mem::take(&mut self.pending);
        for &(idx, plan, scope, _) in pending.iter().filter(|p| !p.3) {
            self.place(idx, plan, scope)?;
        }
        let collects = std::mem::take(&mut self.collects);
        for comp in collects {
            self.expand_collect(comp)?;
        }
        let pointlists = std::mem::take(&mut self.pointlists);
        for comp in pointlists {
            self.expand_pointlist(comp)?;
        }
        for &(idx, plan, scope, _) in pending.iter().filter(|p| p.3) {
            self.place(idx, plan, scope)?;
        }
        for i in 0..self.instances.len() {
            let inst = self.instances[i];
            self.instance_sources(inst)?;
        }
        Ok(())
    }

    /// Give a `$ref` child its kind: a copy of a component, a number aliasing
    /// one prop, or a number holding the missing-referent cell.
    fn place(&mut self, idx: CompIdx, plan: PlanId, scope: ScopeId) -> Result<()> {
        let (target, prop) = self.resolve(plan, scope)?;
        match (target, prop) {
            (Resolved::Missing, _) => {
                self.comps.kind[idx as usize] = ComponentKind::Number;
                self.allocate_slots(idx, 1);
                let s = self.slot(idx, 0);
                self.sources[s as usize] = Source::Alias(self.missing_slot());
            }
            (_, Some(prop)) => {
                let targets = self.targets_of(target, Some(prop), plan, Some(1))?;
                self.comps.kind[idx as usize] = ComponentKind::Number;
                self.allocate_slots(idx, 1);
                let s = self.slot(idx, 0);
                self.sources[s as usize] = Source::Alias(targets[0]);
            }
            (target, None) => {
                let referent = self.single_component(target, plan)?;
                let kind = self.comps.kind[referent as usize];
                if !kind.copyable() {
                    return Err(Error::UncopyableKind(kind.tag().into()));
                }
                self.copy_into(idx, referent);
            }
        }
        Ok(())
    }

    /// Make `idx` a copy of `referent`: same kind, every slot aliased.
    fn copy_into(&mut self, idx: CompIdx, referent: CompIdx) {
        let kind = self.comps.kind[referent as usize];
        self.comps.kind[idx as usize] = kind;
        self.allocate_slots(idx, kind.prop_defs().len());
        for pi in 0..kind.prop_defs().len() {
            let s = self.slot(idx, pi);
            self.sources[s as usize] = Source::Alias(self.slot(referent, pi));
        }
    }

    /// The coordinate slots of each item of an array prop a plan names
    /// (`$l.points`, `$pg.vertices`), trimmed to the live item count.
    fn resolve_items(&mut self, plan: PlanId, scope: ScopeId) -> Result<Vec<[SlotId; 2]>> {
        let (target, prop) = self.resolve(plan, scope)?;
        let comp = self.single_component(target, plan)?;
        let kind = self.comps.kind[comp as usize];
        let prop = prop.ok_or_else(|| Error::PathTooDeep(self.c.plans[plan].display.clone()))?;
        let items = kind.array_prop(prop).ok_or_else(|| Error::UnknownProp { name: self.comp_label(comp), prop: prop.into() })?;
        let count_slot = match kind {
            ComponentKind::Polygon => Some(self.slot(comp, 0)),
            ComponentKind::Circle => Some(self.slot(comp, kind.prop_index("numThroughPoints").unwrap())),
            _ => None,
        };
        let live = match count_slot.map(|s| self.sources[s as usize].clone()) {
            Some(Source::Fixed(n)) => n as usize,
            _ => items.len(),
        };
        Ok(items.iter().take(live).map(|[x, y]| [self.slot(comp, kind.prop_index(x).unwrap()), self.slot(comp, kind.prop_index(y).unwrap())]).collect())
    }

    /// Give a point list its children: one synthesized point per item of
    /// the array prop it extends, or copies of the children of the point
    /// list it is a copy of.
    fn expand_pointlist(&mut self, comp: CompIdx) -> Result<()> {
        let inst = self.instances[self.comp_instance[comp as usize] as usize];
        let el = &self.c.templates[inst.template].elems[inst.elem];
        let mut kids = Vec::new();
        match (el.body.clone(), el.extend) {
            (Body::PointList { from }, _) => {
                let items = self.resolve_items(from, inst.scope)?;
                for [x, y] in items {
                    let pt = self.new_component(ComponentKind::Point, NONE, comp, NONE, inst.scope, 3);
                    let (sx, sy, sh) = (self.slot(pt, 0), self.slot(pt, 1), self.slot(pt, 2));
                    self.sources[sx as usize] = Source::Alias(x);
                    self.sources[sy as usize] = Source::Alias(y);
                    // Not state: a synthesized point has no essential key.
                    self.sources[sh as usize] = Source::Fixed(0.0);
                    kids.push(pt);
                }
            }
            (_, Some(p)) => {
                let (target, _) = self.resolve(p, inst.scope)?;
                let referent = self.single_component(target, p)?;
                let (s, n) = (self.comps.child_start[referent as usize] as usize, self.comps.child_count[referent as usize] as usize);
                let originals: Vec<CompIdx> = self.comps.child_list[s..s + n].to_vec();
                for orig in originals {
                    let copy = self.new_component(ComponentKind::Document, NONE, comp, NONE, inst.scope, 0);
                    self.copy_into(copy, orig);
                    kids.push(copy);
                }
            }
            _ => {}
        }
        self.set_children(comp, &kids);
        Ok(())
    }

    fn expand_collect(&mut self, comp: CompIdx) -> Result<()> {
        let inst = self.instances[self.comp_instance[comp as usize] as usize];
        let Body::Collect { from, kind } = self.c.templates[inst.template].elems[inst.elem].body else { unreachable!() };
        let (target, _) = self.resolve(from, inst.scope)?;
        let mut found = Vec::new();
        match target {
            Resolved::Comp(c) => self.collect_descendants(c, kind, &mut found),
            Resolved::Iter(repeat, s) => {
                for c in self.iteration_components(repeat, s) {
                    if self.comps.kind[c as usize] == kind {
                        found.push(c);
                    } else {
                        self.collect_descendants(c, kind, &mut found);
                    }
                }
            }
            Resolved::Missing => {}
        }
        let mut copies = Vec::with_capacity(found.len());
        for &c in &found {
            // Created kind-less so `copy_into` allocates the slots once.
            let copy = self.new_component(ComponentKind::Document, NONE, comp, NONE, inst.scope, 0);
            self.copy_into(copy, c);
            copies.push(copy);
        }
        self.set_children(comp, &copies);
        let s0 = self.slot(comp, 0);
        self.sources[s0 as usize] = Source::Fixed(copies.len() as f64);
        self.collected.insert(comp, copies);
        Ok(())
    }

    /// Descendants of `c` of `kind`, in document order, not recursing into
    /// a match (as the current core's `recurseToMatchedChildren: false`).
    fn collect_descendants(&self, c: CompIdx, kind: ComponentKind, out: &mut Vec<CompIdx>) {
        let (s, n) = (self.comps.child_start[c as usize] as usize, self.comps.child_count[c as usize] as usize);
        for i in s..s + n {
            let e = self.comps.child_list[i];
            if e & TEXT_BIT != 0 {
                continue;
            }
            if self.comps.kind[e as usize] == kind {
                out.push(e);
            } else {
                self.collect_descendants(e, kind, out);
            }
        }
    }

    /// The template components of one iteration: children of the repeat
    /// created in that scope.
    fn iteration_components(&self, repeat: CompIdx, scope: ScopeId) -> Vec<CompIdx> {
        let (s, n) = (self.comps.child_start[repeat as usize] as usize, self.comps.child_count[repeat as usize] as usize);
        self.comps.child_list[s..s + n].iter().copied().filter(|&e| e & TEXT_BIT == 0 && self.comps.scope[e as usize] == scope).collect()
    }

    /// Sources for every prop of one instantiated element.
    fn instance_sources(&mut self, inst: Instance) -> Result<()> {
        let comp = inst.comp;
        let (kind, extend_plan, is_math) = {
            let el = &self.c.templates[inst.template].elems[inst.elem];
            (el.kind, el.extend, matches!(el.body, Body::Math(_)))
        };
        let extend = match extend_plan {
            Some(p) => {
                let (target, _) = self.resolve(p, inst.scope)?;
                let referent = self.single_component(target, p)?;
                let rk = self.comps.kind[referent as usize];
                if rk != kind {
                    return Err(Error::ExtendKindMismatch { referent: self.c.plans[p].display.clone(), referent_kind: rk.tag().into(), kind: kind.tag().into() });
                }
                Some(referent)
            }
            None => None,
        };
        if is_math {
            let Body::Math(expr) = self.c.templates[inst.template].elems[inst.elem].body else { unreachable!() };
            let id = self.instantiate_expr(expr, inst.scope)?;
            let expr_slot = self.slot(comp, 0);
            let value_slot = self.slot(comp, 1);
            self.sources[expr_slot as usize] = Source::Fixed(id as f64);
            self.sources[value_slot as usize] = if let Expr::Num(v) = *self.arena.get(id) {
                // `<math>5</math>` is state, as a number literal is: a drag
                // that reaches it changes it, as in the current core.
                Source::Literal(v)
            } else if self.arena.is_numeric(id) {
                Source::Alias(self.lower(id))
            } else {
                // The handle, then the expression's cell leaves as extra inputs.
                let start = self.op_inputs.len() as u32;
                self.op_inputs.push(expr_slot);
                let mut leaves = Vec::new();
                self.arena.cell_leaves(id, &mut leaves);
                self.op_inputs.extend_from_slice(&leaves);
                Source::Op(OpSpec::Evaluate, start, 1 + leaves.len() as u8)
            };
            return Ok(());
        }
        let n_props = self.c.templates[inst.template].elems[inst.elem].props.len();
        // A copy of a planned kind aliases its public props and has no
        // hidden slots of its own.
        for pi in 0..n_props {
            let s = self.slot(comp, pi);
            if !matches!(self.sources[s as usize], Source::Unset) {
                continue; // set during expansion (a collect's count)
            }
            // `self.c` is a shared borrow independent of `self`'s own fields,
            // so plans are read in place while sources are written.
            let c: &'c Compiled<'a> = self.c;
            let source = match &c.templates[inst.template].elems[inst.elem].props[pi] {
                SourcePlan::Literal(v) => Source::Literal(*v),
                SourcePlan::Default(v) => match extend {
                    Some(r) => Source::Alias(self.slot(r, pi)),
                    None => Source::Default(*v),
                },
                SourcePlan::Fixed(v) => Source::Fixed(*v),
                SourcePlan::Ref(p) => Source::Alias(self.resolve_one(*p, inst.scope).map_err(|e| arity_error(e, kind, kind.prop_defs()[pi].name))?),
                SourcePlan::RefPart(p, i, n) => {
                    let targets = self.resolve_ref(*p, inst.scope, Some(*n))?;
                    if targets.len() != *n {
                        return Err(Error::ArityMismatch { kind: kind.tag().into(), prop: "coords".into(), expected: *n, got: targets.len() });
                    }
                    Source::Alias(targets[*i])
                }
                SourcePlan::Op(spec, args) => {
                    let start = self.op_inputs.len() as u32;
                    for &p in args {
                        let t = self.resolve_one(p, inst.scope).map_err(|e| arity_error(e, ComponentKind::Op, "args"))?;
                        self.op_inputs.push(t);
                    }
                    Source::Op(*spec, start, args.len() as u8)
                }
                SourcePlan::Computed(op, args) => {
                    let start = self.op_inputs.len() as u32;
                    for &a in args {
                        self.op_inputs.push(self.slot(comp, a as usize));
                    }
                    Source::Op(*op, start, args.len() as u8)
                }
                SourcePlan::AliasOwn(a) => Source::Alias(self.slot(comp, *a as usize)),
                SourcePlan::IterIndex => Source::Fixed(self.scopes[inst.scope as usize].2 as f64),
                SourcePlan::Math(expr) => {
                    let id = self.instantiate_expr(*expr, inst.scope)?;
                    if self.arena.is_numeric(id) { Source::Alias(self.lower(id)) } else { Source::Fixed(f64::NAN) }
                }
                SourcePlan::Vec(op, args) => {
                    let start = self.op_inputs.len() as u32;
                    for &a in args {
                        self.op_inputs.push(self.slot(comp, a as usize));
                    }
                    Source::OpVec(*op, start, args.len() as u8)
                }
                SourcePlan::VecOut(head, k) => Source::OutputOf(self.slot(comp, *head as usize), *k),
                SourcePlan::RefItem(p, i, j) => {
                    let items = self.resolve_items(*p, inst.scope)?;
                    match items.get(*i) {
                        Some(item) => Source::Alias(item[*j]),
                        None => Source::Alias(self.missing_slot()),
                    }
                }
                SourcePlan::AliasElem(el, slot) => {
                    let other = self.scope_comps[inst.scope as usize][*el];
                    Source::Alias(self.slot(other, *slot as usize))
                }
            };
            self.sources[s as usize] = source;
        }
        if kind == ComponentKind::Evaluate {
            self.evaluate_sources(comp);
        }
        Ok(())
    }

    /// `<evaluate>`: add the function expression's cell leaves to the
    /// `EvalAt` instruction's inputs, so changes to them propagate.
    fn evaluate_sources(&mut self, idx: CompIdx) {
        let (func, input, value) = (self.slot(idx, 0), self.slot(idx, 1), self.slot(idx, 2));
        let mut root = func;
        while let Source::Alias(t) = self.sources[root as usize] {
            root = t;
        }
        let Source::Fixed(handle) = self.sources[root as usize] else {
            return;
        };
        if handle.is_nan() {
            return;
        }
        let mut leaves = Vec::new();
        self.arena.cell_leaves(handle as ExprId, &mut leaves);
        let start = self.op_inputs.len() as u32;
        self.op_inputs.push(func);
        self.op_inputs.push(input);
        self.op_inputs.extend_from_slice(&leaves);
        self.sources[value as usize] = Source::Op(OpSpec::EvalAt, start, 2 + leaves.len() as u8);
    }

    /// Copy an expression template into the document's arena with its plan
    /// leaves resolved to slots in `scope`.
    fn instantiate_expr(&mut self, id: ExprId, scope: ScopeId) -> Result<ExprId> {
        let e = self.c.arena.get(id).clone();
        let out = match e {
            Expr::Num(v) => Expr::Num(v),
            Expr::Sym(s) => Expr::Sym(s),
            Expr::Cell(plan) => {
                let display = &self.c.plans[plan as usize].display;
                let slot = self.resolve_one(plan as PlanId, scope).map_err(|e| match e {
                    Error::ArityMismatch { .. } => Error::BadMath { text: display.clone(), reason: "a reference inside math must name one cell".into() },
                    other => other,
                })?;
                Expr::Cell(slot)
            }
            Expr::Add(a, b) => Expr::Add(self.instantiate_expr(a, scope)?, self.instantiate_expr(b, scope)?),
            Expr::Sub(a, b) => Expr::Sub(self.instantiate_expr(a, scope)?, self.instantiate_expr(b, scope)?),
            Expr::Mul(a, b) => Expr::Mul(self.instantiate_expr(a, scope)?, self.instantiate_expr(b, scope)?),
            Expr::Div(a, b) => Expr::Div(self.instantiate_expr(a, scope)?, self.instantiate_expr(b, scope)?),
            Expr::Pow(a, b) => Expr::Pow(self.instantiate_expr(a, scope)?, self.instantiate_expr(b, scope)?),
            Expr::Neg(a) => Expr::Neg(self.instantiate_expr(a, scope)?),
        };
        Ok(self.arena.push(out))
    }

    /// Lower a numeric expression to operator slots. Literals fold into
    /// `Scale`/`Offset` parameters where an operator has one; otherwise they
    /// become fixed cells.
    fn lower(&mut self, id: ExprId) -> SlotId {
        let num = |a: &Arena, e: ExprId| match a.get(e) {
            Expr::Num(v) => Some(*v),
            _ => None,
        };
        let e = self.arena.get(id).clone();
        match e {
            Expr::Num(v) => self.anon_slot(Source::Fixed(v)),
            Expr::Cell(slot) => slot,
            Expr::Sym(_) => unreachable!("lowering a symbolic expression"),
            Expr::Neg(a) => {
                let a = self.lower(a);
                self.op_slot(OpSpec::Negate, &[a])
            }
            Expr::Add(a, b) => match (num(&self.arena, a), num(&self.arena, b)) {
                (_, Some(k)) => {
                    let a = self.lower(a);
                    self.op_slot(OpSpec::Offset { k }, &[a])
                }
                (Some(k), _) => {
                    let b = self.lower(b);
                    self.op_slot(OpSpec::Offset { k }, &[b])
                }
                _ => {
                    let (a, b) = (self.lower(a), self.lower(b));
                    self.op_slot(OpSpec::Add, &[a, b])
                }
            },
            Expr::Sub(a, b) => match (num(&self.arena, a), num(&self.arena, b)) {
                (_, Some(k)) => {
                    let a = self.lower(a);
                    self.op_slot(OpSpec::Offset { k: -k }, &[a])
                }
                (Some(k), _) => {
                    let b = self.lower(b);
                    let nb = self.op_slot(OpSpec::Negate, &[b]);
                    self.op_slot(OpSpec::Offset { k }, &[nb])
                }
                _ => {
                    let (a, b) = (self.lower(a), self.lower(b));
                    self.op_slot(OpSpec::Sub, &[a, b])
                }
            },
            Expr::Mul(a, b) => match (num(&self.arena, a), num(&self.arena, b)) {
                (_, Some(k)) => {
                    let a = self.lower(a);
                    self.op_slot(OpSpec::Scale { k }, &[a])
                }
                (Some(k), _) => {
                    let b = self.lower(b);
                    self.op_slot(OpSpec::Scale { k }, &[b])
                }
                _ => {
                    let (a, b) = (self.lower(a), self.lower(b));
                    self.op_slot(OpSpec::Mul, &[a, b])
                }
            },
            Expr::Div(a, b) => match num(&self.arena, b) {
                Some(k) => {
                    let a = self.lower(a);
                    self.op_slot(OpSpec::Scale { k: 1.0 / k }, &[a])
                }
                None => {
                    let (a, b) = (self.lower(a), self.lower(b));
                    self.op_slot(OpSpec::Div, &[a, b])
                }
            },
            Expr::Pow(a, b) => {
                let (a, b) = (self.lower(a), self.lower(b));
                self.op_slot(OpSpec::Pow, &[a, b])
            }
        }
    }

    fn op_slot(&mut self, spec: OpSpec, inputs: &[SlotId]) -> SlotId {
        let start = self.op_inputs.len() as u32;
        self.op_inputs.extend_from_slice(inputs);
        self.anon_slot(Source::Op(spec, start, inputs.len() as u8))
    }

    // ---- reference resolution at expansion time ------------------------------

    /// Walk a plan from `scope`. Returns where it arrived and the prop it
    /// named, if any. Every step is an array read.
    fn resolve(&self, plan: PlanId, scope: ScopeId) -> Result<(Resolved, Option<&'c str>)> {
        let p = &self.c.plans[plan];
        let mut sc = scope;
        for _ in 0..p.hops {
            sc = self.scopes[sc as usize].0;
        }
        let mut cur = Resolved::Iter(NONE, sc);
        for step in &p.steps {
            match step {
                Step::Elem(e) => {
                    cur = match cur {
                        Resolved::Iter(_, s) => {
                            sc = s;
                            Resolved::Comp(self.scope_comps[s as usize][*e])
                        }
                        Resolved::Missing => Resolved::Missing,
                        // A name inside a named component: same scope.
                        Resolved::Comp(_) => Resolved::Comp(self.scope_comps[sc as usize][*e]),
                    };
                }
                Step::Index(ip) => {
                    let k = self.index_value(ip, scope);
                    cur = match cur {
                        Resolved::Missing => Resolved::Missing,
                        Resolved::Iter(..) => return Err(Error::NotIndexable(p.display.clone())),
                        Resolved::Comp(c) => match self.comps.kind[c as usize] {
                            ComponentKind::RepeatForSequence => {
                                let r = &self.repeats[self.comp_repeat[c as usize] as usize];
                                if k < 1 || k > r.n as i64 { Resolved::Missing } else { Resolved::Iter(c, r.iter_scopes[(k - 1) as usize]) }
                            }
                            ComponentKind::Collect => match self.collected.get(&c) {
                                Some(items) if k >= 1 && (k as usize) <= items.len() => Resolved::Comp(items[(k - 1) as usize]),
                                Some(_) => Resolved::Missing,
                                None => return Err(Error::NotIndexable(p.display.clone())),
                            },
                            ComponentKind::PointList => {
                                let (s, n) = (self.comps.child_start[c as usize] as usize, self.comps.child_count[c as usize] as usize);
                                if k >= 1 && (k as usize) <= n { Resolved::Comp(self.comps.child_list[s + k as usize - 1]) } else { Resolved::Missing }
                            }
                            _ => return Err(Error::NotIndexable(p.display.clone())),
                        },
                    };
                }
            }
        }
        Ok((cur, p.prop.as_deref()))
    }

    fn index_value(&self, ip: &IndexPlan, scope: ScopeId) -> i64 {
        let mut total = 0i64;
        for t in &ip.terms {
            total += match t {
                IndexTerm::Const(c) => *c,
                IndexTerm::Iter(hops) => {
                    let mut s = scope;
                    for _ in 0..*hops {
                        s = self.scopes[s as usize].0;
                    }
                    self.scopes[s as usize].2 as i64
                }
            };
        }
        total
    }

    /// The one component an unindexed path or `$r[k]` denotes.
    fn single_component(&self, target: Resolved, plan: PlanId) -> Result<CompIdx> {
        match target {
            Resolved::Comp(c) => Ok(c),
            Resolved::Iter(repeat, s) => {
                let comps = self.iteration_components(repeat, s);
                if comps.len() == 1 { Ok(comps[0]) } else { Err(Error::AmbiguousIteration(self.c.plans[plan].display.clone(), comps.len())) }
            }
            Resolved::Missing => Err(Error::UnknownName(self.c.plans[plan].display.clone())),
        }
    }

    /// Slots named by a resolved path, using the default prop when none was given.
    fn targets_of(&mut self, target: Resolved, prop: Option<&str>, plan: PlanId, expected: Option<usize>) -> Result<Vec<SlotId>> {
        let comp = match target {
            Resolved::Missing => {
                let s = self.missing_slot();
                return Ok(vec![s; expected.unwrap_or(1)]);
            }
            other => self.single_component(other, plan)?,
        };
        let kind = self.comps.kind[comp as usize];
        let prop = match prop {
            Some(p) => p,
            None => kind.default_prop().ok_or_else(|| Error::NoDefaultProp(self.comp_label(comp)))?,
        };
        if let Some(parts) = kind.virtual_prop(prop) {
            return Ok(parts.iter().map(|p| self.slot(comp, kind.prop_index(p).unwrap())).collect());
        }
        let pi = kind.prop_index(prop).ok_or_else(|| Error::UnknownProp { name: self.comp_label(comp), prop: prop.into() })?;
        Ok(vec![self.slot(comp, pi)])
    }

    fn resolve_ref(&mut self, plan: PlanId, scope: ScopeId, expected: Option<usize>) -> Result<Vec<SlotId>> {
        let (target, prop) = self.resolve(plan, scope)?;
        self.targets_of(target, prop, plan, expected)
    }

    /// The one slot a reference names, without allocating. Errors with a
    /// placeholder `ArityMismatch` (callers fill in kind and prop) when the
    /// reference names several cells.
    fn resolve_one(&mut self, plan: PlanId, scope: ScopeId) -> Result<SlotId> {
        let (target, prop) = self.resolve(plan, scope)?;
        let comp = match target {
            Resolved::Missing => return Ok(self.missing_slot()),
            other => self.single_component(other, plan)?,
        };
        let kind = self.comps.kind[comp as usize];
        let prop = match prop {
            Some(p) => p,
            None => kind.default_prop().ok_or_else(|| Error::NoDefaultProp(self.comp_label(comp)))?,
        };
        if let Some(parts) = kind.virtual_prop(prop) {
            return Err(Error::ArityMismatch { kind: String::new(), prop: String::new(), expected: 1, got: parts.len() });
        }
        let pi = kind.prop_index(prop).ok_or_else(|| Error::UnknownProp { name: self.comp_label(comp), prop: prop.into() })?;
        Ok(self.slot(comp, pi))
    }

    // ---- cells and program ---------------------------------------------------

    fn finish(mut self) -> Result<Unscheduled> {
        let profile = std::env::var_os("CELLS_BUILD_PROFILE").is_some();
        let clock = web_time::Instant::now();
        let lap = |what: &str| {
            if profile {
                eprintln!("    cd/{what}: {:.2?}", clock.elapsed());
            }
        };
        let n = self.sources.len();
        let mut uf = UnionFind::new(n);
        for (s, src) in self.sources.iter().enumerate() {
            if let Source::Alias(t) = src {
                uf.union(s as SlotId, *t);
            }
        }
        // The one non-alias slot in each class defines its cell.
        let mut class_def: Vec<u32> = vec![NONE; n];
        for (s, src) in self.sources.iter().enumerate() {
            match src {
                Source::Alias(_) => {}
                Source::Unset => unreachable!("slot {} never received a source", self.slot_label(s as SlotId)),
                _ => {
                    let root = uf.find(s as SlotId) as usize;
                    debug_assert!(class_def[root] == NONE, "two sources in one alias class");
                    class_def[root] = s as u32;
                }
            }
        }
        lap("union-find");

        // Number cells: essential classes first, then fixed, then derived.
        // Essential values come from the prior build when the slot existed.
        let roots: Vec<u32> = (0..n as SlotId).map(|s| uf.find(s)).collect();
        if let Some(s) = (0..n).find(|&s| class_def[roots[s] as usize] == NONE) {
            return Err(Error::Cycle(self.slot_label(s as SlotId)));
        }
        let mut slot_cell: Vec<CellIdx> = vec![NONE; n];
        let mut cells = Vec::with_capacity(n);
        let mut cell_def_slot: Vec<SlotId> = Vec::with_capacity(n);
        let mut essential_slots: Vec<(ScopeId, u32)> = Vec::new();
        let mut fixed_defs = Vec::new();
        let mut derived_defs = Vec::new();
        let mut outputs = Vec::new();
        for s in 0..n {
            let root = roots[s] as usize;
            if class_def[root] as usize != s {
                continue; // not the defining slot of its class
            }
            match &self.sources[s] {
                Source::Literal(v) | Source::Default(v) => {
                    let (scope, tslot) = self.template_slot(s as SlotId);
                    let value = self.prior.values.get(scope as usize).and_then(|row| row.get(tslot as usize).copied().flatten()).unwrap_or(*v);
                    slot_cell[root] = cells.len() as CellIdx;
                    cells.push(value);
                    cell_def_slot.push(s as SlotId);
                    essential_slots.push((scope, tslot));
                }
                Source::Fixed(_) => fixed_defs.push(s),
                Source::Op(..) | Source::OpVec(..) => derived_defs.push(s),
                Source::OutputOf(..) => outputs.push(s),
                _ => unreachable!(),
            }
        }
        let n_essential = cells.len();
        for &s in &fixed_defs {
            let Source::Fixed(v) = self.sources[s] else { unreachable!() };
            let root = roots[s] as usize;
            slot_cell[root] = cells.len() as CellIdx;
            cells.push(v);
            cell_def_slot.push(s as SlotId);
        }
        let n_fixed = fixed_defs.len();
        // A vector instruction's outputs are consecutive cells after its head.
        for &s in &derived_defs {
            let root = roots[s] as usize;
            slot_cell[root] = cells.len() as CellIdx;
            let n_out = match self.sources[s] {
                Source::OpVec(v, ..) => v.n_out(),
                _ => 1,
            };
            for _ in 0..n_out {
                cells.push(f64::NAN);
                cell_def_slot.push(s as SlotId);
            }
        }
        for &s in &outputs {
            let Source::OutputOf(head, k) = self.sources[s] else { unreachable!() };
            let head_cell = slot_cell[roots[head as usize] as usize];
            debug_assert!(head_cell != NONE, "vector output before its head");
            slot_cell[roots[s] as usize] = head_cell + k as CellIdx;
            cell_def_slot[(head_cell + k as CellIdx) as usize] = s as SlotId;
        }
        let slot_to_cell: Vec<CellIdx> = roots.iter().map(|&r| slot_cell[r as usize]).collect();
        lap("number cells");

        let mut instrs = Vec::with_capacity(derived_defs.len());
        let mut extra = Vec::new();
        let mut bound: Vec<CellIdx> = Vec::with_capacity(8);
        for &s in &derived_defs {
            let (spec, start, count) = match &self.sources[s] {
                Source::Op(spec, start, count) => (*spec, *start, *count),
                Source::OpVec(v, start, count) => (OpSpec::Vec(*v), *start, *count),
                _ => unreachable!(),
            };
            bound.clear();
            bound.extend(self.op_inputs[start as usize..start as usize + count as usize].iter().map(|&i| slot_to_cell[i as usize]));
            instrs.push(Instr { out: slot_to_cell[s], op: spec.bind(&bound, &mut extra) });
        }
        lap("bind instructions");

        // Prop cells: contiguous per component, in slot order.
        let n_comps = self.comps.len();
        self.comps.prop_base = Vec::with_capacity(n_comps);
        self.comps.prop_cells = Vec::with_capacity(n);
        for c in 0..n_comps {
            let np = self.comps.kind[c].prop_defs().len();
            self.comps.prop_base.push(self.comps.prop_cells.len() as u32);
            let base = self.slot_base[c] as usize;
            self.comps.prop_cells.extend_from_slice(&slot_to_cell[base..base + np]);
        }
        lap("prop cells");

        let (depths, cross_reads) = self.structural_depths();
        lap("structural depth");

        // Cell leaves in the arena were slots; they are cells now.
        let mut arena = self.arena;
        arena.map_cells(|slot| slot_to_cell[slot as usize]);

        // The value store grows with the scope table; rows fill lazily.
        let mut values = self.prior.values.clone();
        values.resize(self.scopes.len(), Vec::new());

        let structure = Structure {
            scopes: self.scopes,
            scope_index: self.scope_index,
            essential_slots,
            values,
            structural_depth: depths.iter().copied().max().unwrap_or(0),
            repeat_depths: depths,
            repeat_cross_reads: cross_reads,
            repeats: self.repeats,
            counts_used: self.counts_used,
        };

        // Lazy labels for cycle errors.
        let slot_comp = self.slot_comp;
        let slot_base = self.slot_base;
        let kinds = self.comps.kind.clone();
        let names: Vec<Option<String>> = self.comps.name.iter().map(|&s| (s != NONE).then(|| self.c.dast.strings.get(s).trim().to_string())).collect();
        let cell_label = Box::new(move |cell: CellIdx| {
            let slot = cell_def_slot[cell as usize];
            let comp = slot_comp[slot as usize];
            if comp == NONE {
                return "(anonymous cell)".to_string();
            }
            let pi = (slot - slot_base[comp as usize]) as usize;
            let owner = names[comp as usize].clone().unwrap_or_else(|| format!("<{}>#{}", kinds[comp as usize].tag(), comp));
            match kinds[comp as usize].prop_defs().get(pi) {
                Some(def) => format!("{owner}.{}", def.name),
                None => format!("{owner}.(hidden slot {pi})"),
            }
        });

        let mut comps = self.comps;
        comps.kind.shrink_to_fit();
        comps.name.shrink_to_fit();
        comps.parent.shrink_to_fit();
        comps.child_start.shrink_to_fit();
        comps.child_count.shrink_to_fit();
        comps.child_list.shrink_to_fit();
        comps.prop_cells.shrink_to_fit();
        comps.node.shrink_to_fit();
        comps.scope.shrink_to_fit();
        Ok(Unscheduled { cells, n_essential, n_fixed, instrs, comps, strings: self.c.dast.strings.clone(), root: self.root, structure, arena, extra, cell_label })
    }

    /// The (scope, template slot) an essential slot's value is saved under.
    /// Only template elements have essential slots; copies alias theirs.
    fn template_slot(&self, slot: SlotId) -> (ScopeId, u32) {
        let comp = self.slot_comp[slot as usize];
        let inst = self.instances[self.comp_instance[comp as usize] as usize];
        let pi = slot - self.slot_base[comp as usize];
        (inst.scope, self.c.templates[inst.template].elems[inst.elem].slot_off + pi)
    }

    /// Structural depth per repeat: how many repeats must be expanded, in
    /// sequence, before its count can be computed, plus one. Nesting adds
    /// one; a count that reads a cell inside another repeat's iterations adds
    /// one, and is flagged (`cross`) since authors can avoid it.
    fn structural_depths(&self) -> (Vec<u32>, Vec<bool>) {
        let count_pi = ComponentKind::RepeatForSequence.prop_index("count").unwrap();
        let mut owner: HashMap<ScopeId, usize> = HashMap::new();
        for (ri, r) in self.repeats.iter().enumerate() {
            for &s in &r.iter_scopes {
                owner.insert(s, ri);
            }
        }
        let is_ancestor_or_self = |s: ScopeId, of: ScopeId| -> bool {
            let mut cur = of;
            loop {
                if cur == s {
                    return true;
                }
                if cur == 0 {
                    return false;
                }
                cur = self.scopes[cur as usize].0;
            }
        };
        let mut reads: Vec<Vec<usize>> = vec![Vec::new(); self.repeats.len()];
        let mut cross = vec![false; self.repeats.len()];
        let mut seen = vec![false; self.sources.len()];
        let mut stack = Vec::new();
        let mut touched = Vec::new();
        for (ri, r) in self.repeats.iter().enumerate() {
            if let Some(&o) = owner.get(&r.scope) {
                reads[ri].push(o);
            }
            stack.clear();
            stack.push(self.slot(r.comp, count_pi));
            touched.clear();
            while let Some(sl) = stack.pop() {
                if seen[sl as usize] {
                    continue;
                }
                seen[sl as usize] = true;
                touched.push(sl);
                let comp = self.slot_comp[sl as usize];
                if comp != NONE {
                    let scope = self.comps.scope[comp as usize];
                    if scope != 0
                        && !is_ancestor_or_self(scope, r.scope)
                        && let Some(&o) = owner.get(&scope)
                        && o != ri
                    {
                        cross[ri] = true;
                        if !reads[ri].contains(&o) {
                            reads[ri].push(o);
                        }
                    }
                }
                match &self.sources[sl as usize] {
                    Source::Alias(t) => stack.push(*t),
                    Source::Op(_, start, n) | Source::OpVec(_, start, n) => stack.extend_from_slice(&self.op_inputs[*start as usize..*start as usize + *n as usize]),
                    Source::OutputOf(head, _) => stack.push(*head),
                    _ => {}
                }
            }
            for &sl in &touched {
                seen[sl as usize] = false;
            }
        }
        fn depth(ri: usize, reads: &[Vec<usize>], memo: &mut [u32], visiting: &mut [bool]) -> u32 {
            if memo[ri] != 0 {
                return memo[ri];
            }
            if visiting[ri] {
                return 1;
            }
            visiting[ri] = true;
            let d = 1 + reads[ri].iter().map(|&o| depth(o, reads, memo, visiting)).max().unwrap_or(0);
            visiting[ri] = false;
            memo[ri] = d;
            d
        }
        let mut memo = vec![0u32; self.repeats.len()];
        let mut visiting = vec![false; self.repeats.len()];
        let depths = (0..self.repeats.len()).map(|ri| depth(ri, &reads, &mut memo, &mut visiting)).collect();
        (depths, cross)
    }
}

/// `fixed`: the element's essential values become constants.
fn fix_literals(props: &mut [Option<SourcePlan>]) {
    for p in props.iter_mut() {
        if let Some(SourcePlan::Literal(v)) = p {
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

fn virtual_attrs(kind: ComponentKind) -> Vec<(&'static str, &'static [&'static str])> {
    match kind {
        ComponentKind::Point => vec![("coords", kind.virtual_prop("coords").unwrap())],
        _ => vec![],
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
