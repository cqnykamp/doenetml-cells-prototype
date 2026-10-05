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
use crate::document::{CellIdx, CompIdx, Components, Document, Repeat, ScopeId, Structure, NONE, TEXT_BIT};
use crate::error::{Error, Result};
use crate::expr::{Arena, Expr, ExprId, Parser, Token};
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
    Repeat { template: TemplateId },
    Collect { from: PlanId, kind: ComponentKind },
    /// `<math>`: `expr` is the handle, `value` lowers or evaluates.
    Math(ExprId),
}

#[derive(Debug, Clone)]
struct Elem {
    node: NodeId,
    kind: ComponentKind,
    name: StrId,
    /// First of this element's prop slots within the template's slot space.
    slot_off: u32,
    props: Vec<SourcePlan>,
    children: Vec<Child>,
    extend: Option<PlanId>,
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
    /// Compile-time name table.
    names: HashMap<String, ElemId>,
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
            Some(el) => vec![Child::Elem(cp.add_elem(0, el)?)],
            None => {
                // Synthesize a root when the DAST was not normalized.
                let e = cp.add_synthetic(0, ComponentKind::Document, NONE);
                let kids = cp.add_children(0, Some(e), dast.children(Dast::ROOT))?;
                cp.c.templates[0].elems[e].children = kids;
                vec![Child::Elem(e)]
            }
        };
        cp.c.templates[0].children = children;
        // Every name exists now: plan attributes and macro children.
        for (t, e) in std::mem::take(&mut cp.pending_elems) {
            cp.plan_elem(t, e)?;
        }
        for (t, owner, i, m) in std::mem::take(&mut cp.pending_macros) {
            let plan = cp.plan_ref(t, m)?;
            let has_index = dast.macro_has_index(m);
            match owner {
                Some(e) => cp.c.templates[t].elems[e].children[i] = Child::Macro(plan, has_index),
                None => cp.c.templates[t].children[i] = Child::Macro(plan, has_index),
            }
        }
        Ok(cp.c)
    }

    fn push_elem(&mut self, t: TemplateId, node: NodeId, kind: ComponentKind, name: StrId) -> Result<ElemId> {
        let tpl = &mut self.c.templates[t];
        let e = tpl.elems.len();
        tpl.elems.push(Elem { node, kind, name, slot_off: tpl.n_slots, props: Vec::new(), children: Vec::new(), extend: None, body: Body::Plain });
        tpl.n_slots += kind.prop_defs().len() as u32;
        if name != NONE {
            let s = self.c.dast.strings.get(name).trim().to_string();
            if tpl.names.insert(s.clone(), e).is_some() {
                return Err(Error::DuplicateName(s));
            }
        }
        Ok(e)
    }

    fn add_synthetic(&mut self, t: TemplateId, kind: ComponentKind, name: StrId) -> ElemId {
        self.push_elem(t, NONE, kind, name).expect("synthetic names are unique")
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
    fn add_elem(&mut self, t: TemplateId, el: NodeId) -> Result<ElemId> {
        let d = self.c.dast;
        let tag = d.str(el);
        let kind = ComponentKind::from_tag(tag).ok_or_else(|| Error::UnsupportedTag(tag.to_string()))?;
        let name = self.attr_name_str(el, "name").unwrap_or(NONE);
        let e = self.push_elem(t, el, kind, name)?;
        self.pending_elems.push((t, e));
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
                let kids = self.add_children(sub, None, d.children(el))?;
                self.c.templates[sub].children = kids;
                self.c.templates[t].elems[e].body = Body::Repeat { template: sub };
            }
            ComponentKind::Collect => {}
            // A number's children are its value, not rendered children.
            _ if kind.prop_defs().iter().any(|p| p.from == PropFrom::Children) => {}
            _ => {
                let kids = self.add_children(t, Some(e), d.children(el))?;
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

    fn add_children(&mut self, t: TemplateId, owner: Option<ElemId>, nodes: &[NodeId]) -> Result<Vec<Child>> {
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
                    kids.push(Child::Elem(self.add_elem(t, n)?));
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
        let (el, kind) = {
            let x = &self.c.templates[t].elems[e];
            (x.node, x.kind)
        };
        if el == NONE {
            return Ok(());
        }
        let extend = match d.attr(el, "extend") {
            Some(a) => {
                let m = self.single_macro(a).ok_or_else(|| Error::BadValue { attr: "extend".into(), text: self.attr_text(a).unwrap_or_default() })?;
                let p = self.plan_ref(t, m)?;
                if self.c.plans[p].prop.is_some() {
                    return Err(Error::PathTooDeep(d.macro_display(m)));
                }
                Some(p)
            }
            None => None,
        };
        self.c.templates[t].elems[e].extend = extend;

        match kind {
            ComponentKind::Math => {
                let id = self.plan_math(t, d.children(el))?;
                self.c.templates[t].elems[e].body = Body::Math(id);
                return Ok(());
            }
            ComponentKind::Collect => {
                let from = d.attr(el, "from").and_then(|a| self.single_macro(a)).ok_or(Error::BadCollect)?;
                let type_text = d.attr(el, "componentType").and_then(|a| self.attr_text(a)).ok_or(Error::BadCollect)?;
                let ck = ComponentKind::from_tag(type_text.trim()).filter(|k| k.collectable()).ok_or_else(|| Error::BadCollectType(type_text.trim().into()))?;
                let p = self.plan_ref(t, from)?;
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

        let mut props: Vec<Option<SourcePlan>> = vec![None; kind.prop_defs().len()];
        // Virtual multi-cell attributes (a point's coords) bind several props at once.
        for (vname, parts) in virtual_attrs(kind) {
            if let Some(a) = d.attr(el, vname) {
                let m = self.single_macro(a).ok_or_else(|| Error::BadValue { attr: vname.into(), text: self.attr_text(a).unwrap_or_default() })?;
                let p = self.plan_ref(t, m)?;
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
                    Some(SourcePlan::Ref(self.plan_ref(t, m)?))
                }
                None => None,
            };
            let plan = match (bound, def.from) {
                (Some(p), _) => p,
                (None, PropFrom::Attribute) => match d.attr(el, def.attr_name()) {
                    Some(a) => self.plan_value(t, def.attr_name(), d.attr_children(a), def.ref_prop)?,
                    None => SourcePlan::Default(def.default),
                },
                (None, PropFrom::AttributeOr { alias }) => match d.attr(el, def.attr_name()) {
                    Some(a) => self.plan_value(t, def.attr_name(), d.attr_children(a), None)?,
                    None => SourcePlan::AliasOwn(alias),
                },
                (None, PropFrom::Computed { op, args }) => SourcePlan::Computed(op, args.to_vec()),
                (None, PropFrom::Children) => {
                    if d.children(el).iter().all(|&n| self.is_blank(n)) {
                        SourcePlan::Default(def.default)
                    } else {
                        match self.plan_value(t, def.name, d.children(el), None) {
                            Ok(p) => p,
                            // Not a single literal or reference: math text.
                            Err(Error::BadValue { .. }) => SourcePlan::Math(self.plan_math(t, d.children(el))?),
                            Err(e) => return Err(e),
                        }
                    }
                }
                (None, PropFrom::Derived) => self.plan_op(t, el)?,
            };
            props[pi] = Some(plan);
        }
        self.c.templates[t].elems[e].props = props.into_iter().map(|p| p.unwrap()).collect();
        Ok(())
    }

    /// A literal number or a single reference.
    fn plan_value(&mut self, t: TemplateId, attr: &str, nodes: &[NodeId], ref_prop: Option<&str>) -> Result<SourcePlan> {
        let d = self.c.dast;
        let macros: Vec<NodeId> = nodes.iter().copied().filter(|&n| d.kind(n) == NodeKind::Macro).collect();
        let text: String = nodes.iter().filter(|&&n| d.kind(n) == NodeKind::Text).map(|&n| d.str(n)).collect();
        let text = text.trim();
        match (macros.len(), text.is_empty()) {
            (1, true) => {
                let p = self.plan_ref(t, macros[0])?;
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

    fn plan_op(&mut self, t: TemplateId, el: NodeId) -> Result<SourcePlan> {
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
                    NodeKind::Macro => args.push(self.plan_ref(t, node)?),
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
    fn plan_math(&mut self, t: TemplateId, nodes: &[NodeId]) -> Result<ExprId> {
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
                    let p = self.plan_ref(t, n)?;
                    toks.push(Token::Cell(p as CellIdx));
                }
                _ => {}
            }
        }
        Parser::parse(&toks, &mut self.c.arena).map_err(|reason| Error::BadMath { text: text.trim().to_string(), reason })
    }

    // ---- reference plans -----------------------------------------------------

    /// Find a name from template `t` outward; returns (hops, element).
    fn lookup(&self, mut t: TemplateId, name: &str) -> Option<(u32, ElemId)> {
        let mut hops = 0;
        loop {
            if let Some(&e) = self.c.templates[t].names.get(name.trim()) {
                return Some((hops, e));
            }
            let (pt, _) = self.c.templates[t].parent?;
            t = pt;
            hops += 1;
        }
    }

    fn plan_ref(&mut self, t: TemplateId, m: NodeId) -> Result<PlanId> {
        let d = self.c.dast;
        let display = d.macro_display(m);
        let names = d.macro_path(m);
        let parts: Vec<_> = d.macro_parts(m).collect();
        let first = d.strings.get(names[0]);
        let (hops, e0) = self.lookup(t, first).ok_or_else(|| Error::UnknownName(first.into()))?;
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
                        let e = *self.c.templates[cur_t].names.get(name.trim()).ok_or_else(|| Error::UnknownName(display.clone()))?;
                        steps.push(Step::Elem(e));
                        cur_elem = Some(e);
                    }
                    Some(e) => {
                        // A prop name: must be the last part and carry no index.
                        if i + 1 != parts.len() || d.part_indices(part).next().is_some() {
                            return Err(Error::PathTooDeep(display));
                        }
                        let kind = self.c.templates[cur_t].elems[e].kind;
                        let kind = match self.c.templates[cur_t].elems[e].body {
                            // After `$c[k]` the component is a collected copy.
                            Body::Collect { kind: ck, .. } if steps.len() > 1 => ck,
                            _ => kind,
                        };
                        if kind.prop_index(name).is_none() && kind.virtual_prop(name).is_none() {
                            return Err(Error::UnknownProp { name: self.elem_label(cur_t, e), prop: name.into() });
                        }
                        prop = Some(name.to_string());
                        break;
                    }
                }
            }
            for expr in d.part_indices(part) {
                let Some(e) = cur_elem else { return Err(Error::NotIndexable(display)) };
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
                    _ => return Err(Error::NotIndexable(display)),
                }
            }
        }
        self.c.plans.push(RefPlan { hops, steps, prop, display });
        Ok(self.c.plans.len() - 1)
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
                    let (hops, e) = self.lookup(t, name).ok_or_else(|| Error::UnknownName(name.into()))?;
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
            collected: HashMap::new(),
            missing: None,
            arena: Arena::default(),
            root: 0,
        }
    }

    // ---- components and slots ------------------------------------------------

    fn new_component(&mut self, kind: ComponentKind, name: StrId, parent: CompIdx, node: NodeId, scope: ScopeId) -> CompIdx {
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
        self.allocate_slots(idx);
        idx
    }

    fn allocate_slots(&mut self, comp: CompIdx) {
        let n = self.comps.kind[comp as usize].prop_defs().len();
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
        if name != NONE {
            self.c.dast.strings.get(name).trim().to_string()
        } else {
            format!("<{}>#{}", self.comps.kind[comp as usize].tag(), comp)
        }
    }

    fn slot_label(&self, slot: SlotId) -> String {
        let comp = self.slot_comp[slot as usize];
        if comp == NONE {
            return format!("(anonymous slot {slot})");
        }
        let pi = (slot - self.slot_base[comp as usize]) as usize;
        format!("{}.{}", self.comp_label(comp), self.comps.kind[comp as usize].prop_defs()[pi].name)
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
            let comp = self.new_component(el.kind, el.name, parent, el.node, scope);
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
                let idx = self.new_component(ComponentKind::Document, NONE, parent, NONE, scope);
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
                self.allocate_slots(idx);
                let s = self.slot(idx, 0);
                self.sources[s as usize] = Source::Alias(self.missing_slot());
            }
            (_, Some(prop)) => {
                let targets = self.targets_of(target, Some(prop), plan, Some(1))?;
                self.comps.kind[idx as usize] = ComponentKind::Number;
                self.allocate_slots(idx);
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
        self.allocate_slots(idx);
        for pi in 0..kind.prop_defs().len() {
            let s = self.slot(idx, pi);
            self.sources[s as usize] = Source::Alias(self.slot(referent, pi));
        }
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
            let copy = self.new_component(ComponentKind::Document, NONE, comp, NONE, inst.scope);
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
            self.sources[value_slot as usize] = if self.arena.is_numeric(id) {
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
        let Source::Fixed(handle) = self.sources[root as usize] else { return };
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
                        Resolved::Iter(_, s) => Resolved::Comp(self.scope_comps[s as usize][*e]),
                        Resolved::Missing => Resolved::Missing,
                        Resolved::Comp(_) => unreachable!("a name after a component is a prop, not a step"),
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
                Source::Op(..) => derived_defs.push(s),
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
        for &s in &derived_defs {
            let root = roots[s] as usize;
            slot_cell[root] = cells.len() as CellIdx;
            cells.push(f64::NAN);
            cell_def_slot.push(s as SlotId);
        }
        let slot_to_cell: Vec<CellIdx> = roots.iter().map(|&r| slot_cell[r as usize]).collect();
        lap("number cells");

        let mut instrs = Vec::with_capacity(derived_defs.len());
        let mut extra = Vec::new();
        let mut bound: Vec<CellIdx> = Vec::with_capacity(8);
        for &s in &derived_defs {
            let Source::Op(spec, start, count) = &self.sources[s] else { unreachable!() };
            bound.clear();
            bound.extend(self.op_inputs[*start as usize..*start as usize + *count as usize].iter().map(|&i| slot_to_cell[i as usize]));
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
            format!("{owner}.{}", kinds[comp as usize].prop_defs()[pi].name)
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
                    if scope != 0 && !is_ancestor_or_self(scope, r.scope) {
                        if let Some(&o) = owner.get(&scope) {
                            if o != ri {
                                cross[ri] = true;
                                if !reads[ri].contains(&o) {
                                    reads[ri].push(o);
                                }
                            }
                        }
                    }
                }
                match &self.sources[sl as usize] {
                    Source::Alias(t) => stack.push(*t),
                    Source::Op(_, start, n) => stack.extend_from_slice(&self.op_inputs[*start as usize..*start as usize + *n as usize]),
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
