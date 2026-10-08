//! Compile: walk the DAST once per template and plan every element's
//! props (literals, references, operators, math), with name lookup and
//! reference paths. See the module docs in `mod.rs`.

use super::*;

/// The ways an element is planned; see `Compiler::elem_shape`.
enum ElemShape {
    Cloned,
    Synthetic,
    ExtendProp(PlanId),
    ContainerCopy(PlanId),
    Planned(Option<PlanId>),
    Math,
    /// `<function>`, `<derivative>`, `<answer>` (`plan_symbolic`).
    Symbolic,
    Collect,
    /// `<conditionalContent>`, `<select>` (`choice.rs`).
    Choice,
    Text,
    Generic(Option<PlanId>),
}

impl<'a> Compiler<'a> {
    pub(super) fn compile(dast: &'a Dast) -> Result<Compiled<'a>> {
        let mut cp = Compiler { c: Compiled { dast, templates: vec![Template::default()], plans: Vec::new(), arena: Arena::default(), sym_text: HashMap::new(), choices: Vec::new() }, pending_elems: Vec::new(), pending_macros: Vec::new() };
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
        cp.finish_choices()?;
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
    pub(super) fn child_scope(&self, _t: TemplateId, e: ElemId) -> ElemId {
        e
    }

    pub(super) fn push_elem(&mut self, t: TemplateId, node: NodeId, kind: ComponentKind, name: StrId, name_scope: ElemId) -> Result<ElemId> {
        self.push_elem_visible_to(t, node, kind, name, name_scope, ROOT_SCOPE)
    }

    /// `push_elem` whose name is registered only up to ancestor `stop`
    /// (inclusive): a container copy's children are reached through the
    /// copy's name, never bare, so they do not make the original ambiguous.
    pub(super) fn push_elem_visible_to(&mut self, t: TemplateId, node: NodeId, kind: ComponentKind, name: StrId, name_scope: ElemId, stop: ElemId) -> Result<ElemId> {
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

    pub(super) fn add_synthetic(&mut self, t: TemplateId, kind: ComponentKind, name: StrId) -> ElemId {
        self.push_elem(t, NONE, kind, name, ROOT_SCOPE).expect("synthetic names are unique")
    }

    pub(super) fn attr_name_str(&self, el: NodeId, attr: &str) -> Option<StrId> {
        let d = self.c.dast;
        let a = d.attr(el, attr)?;
        match d.attr_children(a) {
            [t] if d.kind(*t) == NodeKind::Text => Some(d.str_id(*t)),
            _ => None,
        }
    }

    /// Create an element (and, for a repeat, its template) from a DAST element.
    pub(super) fn add_elem(&mut self, t: TemplateId, el: NodeId, name_scope: ElemId) -> Result<ElemId> {
        let d = self.c.dast;
        let tag = d.str(el);
        let kind = ComponentKind::from_tag(tag).ok_or_else(|| Error::UnsupportedTag(tag.to_string()))?;
        // `<group rendered="c">` is a conditional content with one case.
        let kind = if kind == ComponentKind::Group && d.attr(el, "rendered").is_some() { ComponentKind::ConditionalContent } else { kind };
        if kind == ComponentKind::Case {
            return Err(Error::Unsupported("<case> outside a <conditionalContent>".into()));
        }
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
                    let mut props = vec![SourcePlan::reference(from), SourcePlan::reference(step), SourcePlan::IterIndex];
                    for def in ComponentKind::SequenceValue.prop_defs().iter().skip(3) {
                        let PropFrom::Computed { op, args } = def.from else { unreachable!() };
                        props.push(SourcePlan::computed(op, args.to_vec()));
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
            ComponentKind::ConditionalContent | ComponentKind::Select => self.add_choice(t, e, el)?,
            // A number's children are its value, not rendered children; the
            // planned kinds read their children themselves (a line's equation,
            // a point's constraints).
            _ if kind.planned() || kind.prop_defs().iter().any(|p| p.from == PropFrom::Children) => {}
            _ => {
                let kids = self.add_children(t, Some(e), child_scope, d.children(el))?;
                self.c.templates[t].elems[e].children = kids;
            }
        }
        Ok(e)
    }

    /// A plan naming prop `prop` of element `e` in the parent template, as
    /// seen from the repeat's own template (one hop up).
    pub(super) fn own_prop_plan(&mut self, e: ElemId, prop: &str) -> PlanId {
        self.c.plans.push(RefPlan { hops: 1, steps: vec![Step::Elem(e)], prop: Some(prop.to_string()), display: format!("(repeat).{prop}") });
        self.c.plans.len() - 1
    }

    pub(super) fn add_children(&mut self, t: TemplateId, owner: Option<ElemId>, name_scope: ElemId, nodes: &[NodeId]) -> Result<Vec<Child>> {
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

    pub(super) fn plan_elem(&mut self, t: TemplateId, e: ElemId) -> Result<()> {
        let d = self.c.dast;
        let (el, kind, scope) = {
            let x = &self.c.templates[t].elems[e];
            (x.node, x.kind, x.name_scope)
        };
        match self.elem_shape(t, e)? {
            // Every public prop aliases the original's; nothing else to plan.
            ElemShape::Cloned => {
                let n = kind.prop_defs().len();
                self.c.templates[t].elems[e].props = vec![SourcePlan::Inherit; n];
                Ok(())
            }
            // Synthesized elements (iteration values) come planned.
            ElemShape::Synthetic => Ok(()),
            ElemShape::ExtendProp(p) => self.plan_extend_prop(t, e, p),
            ElemShape::ContainerCopy(p) => self.plan_container_copy(t, e, p),
            ElemShape::Planned(extend) => self.plan_geo(t, e, extend),
            ElemShape::Math => {
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
                let id = self.plan_sym_math(t, scope, &nodes)?;
                let post = if self.attr_on(el, "expand") {
                    Post::Expand
                } else if self.attr_on(el, "simplify") {
                    Post::Simplify
                } else {
                    Post::None
                };
                self.c.templates[t].elems[e].props = vec![SourcePlan::MathHandle(id, post), SourcePlan::MathValue(id)];
                Ok(())
            }
            ElemShape::Symbolic => self.plan_symbolic(t, e),
            ElemShape::Choice => self.plan_choice(t, e),
            ElemShape::Text => self.plan_text(t, e),
            ElemShape::Collect => {
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
                Ok(())
            }
            ElemShape::Generic(extend) => self.plan_attrs(t, e, extend),
        }
    }

    /// Which way an element is planned, decided from its kind and its
    /// `extend` attribute before any prop is looked at. Records the extend
    /// plan on the element.
    fn elem_shape(&mut self, t: TemplateId, e: ElemId) -> Result<ElemShape> {
        let d = self.c.dast;
        let (el, kind, scope, cloned) = {
            let x = &self.c.templates[t].elems[e];
            (x.node, x.kind, x.name_scope, x.cloned)
        };
        if cloned {
            return Ok(ElemShape::Cloned);
        }
        if el == NONE {
            return Ok(ElemShape::Synthetic);
        }
        let extend = match d.attr(el, "extend") {
            Some(a) => {
                let m = self.single_macro(a).ok_or_else(|| Error::BadValue { attr: "extend".into(), text: self.attr_text(a).unwrap_or_default() })?;
                let p = self.plan_ref(t, scope, m)?;
                // `<point extend="$c.center"/>`, `<math extend="$c.radius"/>`,
                // `<pointList extend="$l.points"/>`: the element's value
                // props alias the named prop.
                if self.c.plans[p].prop.is_some() {
                    return Ok(ElemShape::ExtendProp(p));
                }
                Some(p)
            }
            None => None,
        };
        self.c.templates[t].elems[e].extend = extend;
        if matches!(kind, ComponentKind::ConditionalContent | ComponentKind::Select) {
            if extend.is_some() {
                return Err(Error::Banned(format!("extend on a <{}>: reference its interface names instead", kind.tag())));
            }
            return Ok(ElemShape::Choice);
        }
        Ok(match (kind, extend) {
            (k, Some(p)) if k.container() => ElemShape::ContainerCopy(p),
            (ComponentKind::PointList, _) => return Err(Error::BadValue { attr: "extend".into(), text: "<pointList> needs extend=\"$shape.points\"".into() }),
            (k, _) if k.planned() => ElemShape::Planned(extend),
            (ComponentKind::Math, _) => ElemShape::Math,
            (ComponentKind::Function | ComponentKind::Derivative | ComponentKind::Answer, _) => ElemShape::Symbolic,
            (ComponentKind::Collect, _) => ElemShape::Collect,
            (ComponentKind::Text, None) => ElemShape::Text,
            _ => ElemShape::Generic(extend),
        })
    }

    /// Plans for a kind described by `PropFrom`: attributes, bindings,
    /// computed chains, children.
    pub(super) fn plan_attrs(&mut self, t: TemplateId, e: ElemId, extend: Option<PlanId>) -> Result<()> {
        let d = self.c.dast;
        let (el, kind, scope) = {
            let x = &self.c.templates[t].elems[e];
            (x.node, x.kind, x.name_scope)
        };
        // Without an attribute a prop is the kind's default, or under
        // `extend` the referent's prop.
        let default = |v: f64| if extend.is_some() { SourcePlan::Inherit } else { SourcePlan::Default(v) };
        let mut props: Vec<Option<SourcePlan>> = vec![None; kind.prop_defs().len()];
        for (pi, def) in kind.prop_defs().iter().enumerate() {
            let bound = match def.bind.and_then(|b| d.attr(el, b)) {
                Some(a) => {
                    let bind = def.bind.unwrap();
                    let m = self.single_macro(a).ok_or_else(|| Error::BadValue { attr: bind.into(), text: self.attr_text(a).unwrap_or_default() })?;
                    Some(SourcePlan::reference(self.plan_ref(t, scope, m)?))
                }
                None => None,
            };
            let plan = match (bound, def.from) {
                (Some(p), _) => p,
                (None, PropFrom::Attribute) => match d.attr(el, def.attr_name()) {
                    Some(a) => self.plan_value(t, scope, def.attr_name(), d.attr_children(a), def.ref_prop)?,
                    None => default(def.default),
                },
                (None, PropFrom::AttributeOr { alias }) => match d.attr(el, def.attr_name()) {
                    Some(a) => self.plan_value(t, scope, def.attr_name(), d.attr_children(a), None)?,
                    None => SourcePlan::own(alias),
                },
                (None, PropFrom::Computed { op, args }) => SourcePlan::computed(op, args.to_vec()),
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
                            None => default(def.default),
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
        if kind == ComponentKind::MathInput {
            self.plan_math_input(el, &mut props)?;
        }
        let fix_attrs: &[&str] = if kind == ComponentKind::Graph { &["fixed", "fixAxes"] } else { &["fixed"] };
        match self.plan_fix(t, scope, el, fix_attrs)? {
            Fix::Off => {}
            Fix::Literal => fix_literals(&mut props),
            Fix::Dynamic(flags) => gate_slots(&mut props, flags),
        }
        self.c.templates[t].elems[e].props = props.into_iter().map(|p| p.unwrap()).collect();
        Ok(())
    }

    /// A mathInput bound to a cell (a reference child or `bindValueTo`)
    /// stays a numeric input. Unbound, its `expr` is an essential math cell
    /// holding the prefill (or its children's text) and `value` evaluates it.
    fn plan_math_input(&mut self, el: NodeId, props: &mut [Option<SourcePlan>]) -> Result<()> {
        let d = self.c.dast;
        let nodes = match d.attr(el, "prefill") {
            Some(a) => d.attr_children(a),
            None => d.children(el),
        };
        let bound = matches!(props[0], Some(SourcePlan::Alias(_))) || nodes.iter().any(|&n| d.kind(n) == NodeKind::Macro);
        if bound {
            props[1] = Some(SourcePlan::Fixed(f64::NAN));
            return Ok(());
        }
        let text: String = nodes.iter().filter(|&&n| d.kind(n) == NodeKind::Text).map(|&n| d.str(n)).collect();
        let text = text.trim();
        let tree = if text.is_empty() { None } else { Some(cells_sym::parse::parse(text).map_err(|reason| Error::BadMath { text: text.into(), reason })?) };
        props[1] = Some(SourcePlan::MathEssential(tree));
        props[0] = Some(SourcePlan::Op(OpSpec::Sym(SymKind::Evaluate), vec![Arg::Own(1)]));
        Ok(())
    }

    /// `<function>`, `<derivative>`, `<answer>`. A curve samples over the
    /// x-range of the graph it sits in, else [-10, 10].
    fn plan_symbolic(&mut self, t: TemplateId, e: ElemId) -> Result<()> {
        let d = self.c.dast;
        let (el, kind, scope) = {
            let x = &self.c.templates[t].elems[e];
            (x.node, x.kind, x.name_scope)
        };
        let nodes: Vec<NodeId> = d.children(el).to_vec();
        let id = self.plan_sym_math(t, scope, &nodes)?;
        let mut plan = ElemPlan::new(kind.prop_defs().len());
        match kind {
            ComponentKind::Function | ComponentKind::Derivative => {
                if kind == ComponentKind::Function {
                    plan.set(0, SourcePlan::SymExpr(id, Post::None));
                } else {
                    let of = plan.hidden(SourcePlan::SymExpr(id, Post::None));
                    plan.set(0, SourcePlan::Op(OpSpec::Sym(SymKind::Derivative), vec![Arg::Own(of)]));
                }
                let graph = (scope != ROOT_SCOPE && self.c.templates[t].elems[scope].kind == ComponentKind::Graph).then_some(scope);
                match graph {
                    Some(g) => {
                        plan.set(1, SourcePlan::Alias(Arg::Elem(g, 0)));
                        plan.set(2, SourcePlan::Alias(Arg::Elem(g, 1)));
                    }
                    None => {
                        plan.set(1, SourcePlan::Fixed(-10.0));
                        plan.set(2, SourcePlan::Fixed(10.0));
                    }
                }
                plan.set(3, SourcePlan::Op(OpSpec::Sym(SymKind::Sample), vec![Arg::Own(0), Arg::Own(1), Arg::Own(2)]));
            }
            ComponentKind::Answer => {
                let response = match d.attr(el, "response") {
                    Some(a) => self.plan_value(t, scope, "response", d.attr_children(a), Some("expr"))?,
                    None => SourcePlan::Fixed(f64::NAN),
                };
                plan.set(0, response);
                plan.set(1, SourcePlan::SymExpr(id, Post::None));
                plan.set(2, SourcePlan::MathEssential(None));
                let eq = if self.attr_on(el, "symbolicEquality") { SymKind::EqualsSyntax } else { SymKind::Equals };
                plan.set(3, SourcePlan::Op(OpSpec::Sym(eq), vec![Arg::Own(2), Arg::Own(1)]));
            }
            _ => unreachable!(),
        }
        self.c.templates[t].elems[e].props = plan.finish();
        Ok(())
    }

    /// `<text>`: literal text is a fixed cell holding its string id; a lone
    /// reference aliases another text's value.
    fn plan_text(&mut self, t: TemplateId, e: ElemId) -> Result<()> {
        let d = self.c.dast;
        let (el, scope) = {
            let x = &self.c.templates[t].elems[e];
            (x.node, x.name_scope)
        };
        let nodes: Vec<NodeId> = d.children(el).iter().copied().filter(|&n| !self.is_blank(n)).collect();
        let plan = match nodes.as_slice() {
            [] => SourcePlan::Fixed(f64::NAN),
            [n] if d.kind(*n) == NodeKind::Text => SourcePlan::Fixed(d.str_id(*n) as f64),
            [n] if d.kind(*n) == NodeKind::Macro => SourcePlan::reference(self.plan_ref(t, scope, *n)?),
            _ => return Err(Error::Unsupported("<text> whose content mixes text, references or elements".into())),
        };
        self.c.templates[t].elems[e].props = vec![plan];
        Ok(())
    }

    /// An on/off attribute that may also take a value: present and not
    /// `false` or `none` (`simplify`, `simplify="full"`).
    fn attr_on(&self, el: NodeId, name: &str) -> bool {
        match self.c.dast.attr(el, name) {
            None => false,
            Some(a) => {
                let text = self.attr_text(a).unwrap_or_default();
                !matches!(text.trim().to_ascii_lowercase().as_str(), "false" | "none")
            }
        }
    }

    /// A literal number or a single reference.
    pub(super) fn plan_value(&mut self, t: TemplateId, scope: ElemId, attr: &str, nodes: &[NodeId], ref_prop: Option<&str>) -> Result<SourcePlan> {
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
                Ok(SourcePlan::reference(p))
            }
            (0, false) => match text {
                "true" => Ok(SourcePlan::Literal(1.0)),
                "false" => Ok(SourcePlan::Literal(0.0)),
                _ => text.parse::<f64>().map(SourcePlan::Literal).map_err(|_| Error::BadValue { attr: attr.into(), text: text.into() }),
            },
            _ => Err(Error::BadValue { attr: attr.into(), text: text.into() }),
        }
    }

    pub(super) fn plan_op(&mut self, t: TemplateId, scope: ElemId, el: NodeId) -> Result<SourcePlan> {
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
            "gate" => OpSpec::Gate,
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
        Ok(SourcePlan::Op(spec, args.into_iter().map(|p| Arg::Ref(p, Sel::Whole)).collect()))
    }

    /// Math text and `$ref` children to an expression template whose cell
    /// leaves are plan ids.
    pub(super) fn plan_math(&mut self, t: TemplateId, scope: ElemId, nodes: &[NodeId]) -> Result<ExprId> {
        let (toks, text) = self.math_tokens(t, scope, nodes)?;
        Parser::parse(&toks, &mut self.c.arena).map_err(|reason| Error::BadMath { text, reason })
    }

    /// Math text and `$ref` children to an expression template, recording
    /// the text for the symbolic engine in case the math turns out symbolic.
    pub(super) fn plan_sym_math(&mut self, t: TemplateId, scope: ElemId, nodes: &[NodeId]) -> Result<ExprId> {
        let (toks, text, sym) = self.math_tokens_sym(t, scope, nodes)?;
        let id = Parser::parse(&toks, &mut self.c.arena).map_err(|reason| Error::BadMath { text, reason })?;
        self.c.sym_text.insert(id, sym);
        Ok(id)
    }

    /// Tokenize math text with `$ref` children as cell leaves holding plan ids.
    pub(super) fn math_tokens(&mut self, t: TemplateId, scope: ElemId, nodes: &[NodeId]) -> Result<(Vec<Token>, String)> {
        let (toks, text, _) = self.math_tokens_sym(t, scope, nodes)?;
        Ok((toks, text))
    }

    /// `math_tokens`, plus the text with each `$ref` written `#plan`.
    fn math_tokens_sym(&mut self, t: TemplateId, scope: ElemId, nodes: &[NodeId]) -> Result<(Vec<Token>, String, String)> {
        let d = self.c.dast;
        let mut toks: Vec<Token> = Vec::new();
        let mut text = String::new();
        let mut sym = String::new();
        for &n in nodes {
            match d.kind(n) {
                NodeKind::Text => {
                    text.push_str(d.str(n));
                    sym.push_str(d.str(n));
                    crate::expr::tokenize(d.str(n), &mut toks).map_err(|reason| Error::BadMath { text: text.clone(), reason })?;
                }
                NodeKind::Element if matches!(d.str(n), "conditionalContent" | "select") => {
                    return Err(Error::Unsupported(format!("a <{}> inside math: each branch would have to yield the same type", d.str(n))));
                }
                NodeKind::Macro => {
                    text.push('$');
                    text.push_str(&d.macro_display(n));
                    let p = self.plan_ref(t, scope, n)?;
                    // Spaces keep `2$a` from reading as one token.
                    sym.push_str(&format!(" #{p} "));
                    toks.push(Token::Cell(p as CellIdx));
                }
                _ => {}
            }
        }
        Ok((toks, text.trim().to_string(), sym))
    }

    // ---- planned kinds -------------------------------------------------------
    //
    // Geometric kinds are planned from the whole element: which attributes
    // are present decides which operator chain produces the public props.
    // That is where the current core's build-time variety goes (plan 3).

    /// A copy of plan `p` naming `prop` instead of its own (`$l` -> `$l.x2`).
    pub(super) fn plan_with_prop(&mut self, p: PlanId, prop: &str) -> PlanId {
        let mut plan = self.c.plans[p].clone();
        plan.prop = Some(prop.to_string());
        plan.display = format!("{}.{prop}", plan.display);
        self.c.plans.push(plan);
        self.c.plans.len() - 1
    }

    /// The element a plan names, if it is in template `t` itself (a path of
    /// names without indices, no prop).
    pub(super) fn plan_elem_target(&self, _t: TemplateId, p: PlanId) -> Option<ElemId> {
        let plan = &self.c.plans[p];
        if plan.hops != 0 || plan.prop.is_some() {
            return None;
        }
        let mut last = None;
        for step in &plan.steps {
            match step {
                Step::Elem(e) => last = Some(*e),
                Step::Index(_) | Step::Iface(..) => return None,
            }
        }
        last
    }

    /// `fixed`-like attributes of an element. A literal true fixes it at
    /// build time; a reference is a flag cell, so the element is gated
    /// while the flag is nonzero. Several references gate on any of them.
    pub(super) fn plan_fix(&mut self, t: TemplateId, scope: ElemId, el: NodeId, names: &[&str]) -> Result<Fix> {
        let d = self.c.dast;
        let mut flags = Vec::new();
        for &name in names {
            let Some(a) = d.attr(el, name) else { continue };
            if self.attr_text(a).is_some() {
                if self.attr_flag(el, name) {
                    return Ok(Fix::Literal);
                }
                continue;
            }
            flags.push(self.plan_value(t, scope, name, d.attr_children(a), None)?);
        }
        Ok(if flags.is_empty() { Fix::Off } else { Fix::Dynamic(flags) })
    }

    /// A boolean attribute: present and empty, or `true`.
    pub(super) fn attr_flag(&self, el: NodeId, name: &str) -> bool {
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
    pub(super) fn plan_scalar(&mut self, t: TemplateId, scope: ElemId, attr: &str, nodes: &[NodeId]) -> Result<SourcePlan> {
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
    pub(super) fn plan_from_expr(&mut self, id: ExprId) -> SourcePlan {
        match self.c.arena.get(id) {
            Expr::Num(v) => SourcePlan::Literal(*v),
            Expr::Cell(p) => SourcePlan::reference(*p as PlanId),
            _ => SourcePlan::Math(id),
        }
    }

    /// `(a, b)`: two scalar plans from tuple text, with `$ref` leaves.
    pub(super) fn plan_tuple(&mut self, t: TemplateId, scope: ElemId, nodes: &[NodeId]) -> Result<Vec<SourcePlan>> {
        let d = self.c.dast;
        let macros: Vec<NodeId> = nodes.iter().copied().filter(|&n| d.kind(n) == NodeKind::Macro).collect();
        let text_blank = nodes.iter().all(|&n| d.kind(n) != NodeKind::Text || d.str(n).trim().is_empty());
        if macros.len() == 1 && text_blank {
            // `<point>$q</point>`: alias the referent's coordinates.
            let p = self.plan_ref(t, scope, macros[0])?;
            return Ok(vec![SourcePlan::coord(p, 0), SourcePlan::coord(p, 1)]);
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

    /// Find a name as the current core's resolver does: from the
    /// referencing element `from` (or `ROOT_SCOPE`) walk up the ancestors;
    /// at each, the ancestor's own name wins, then a unique descendant with
    /// the name; several descendants are an ambiguity. Then continue in the
    /// enclosing template from the repeat element. Returns (hops, element).
    pub(super) fn lookup(&self, mut t: TemplateId, mut from: ElemId, name: &str) -> Result<Option<(u32, ElemId)>> {
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
    pub(super) fn child_named(&self, t: TemplateId, e: ElemId, name: &str) -> Result<Option<ElemId>> {
        match self.c.templates[t].names.get(&(e, name.trim().to_string())).map(Vec::as_slice) {
            Some([c]) => Ok(Some(*c)),
            Some([_, _, ..]) => Err(Error::AmbiguousName(name.trim().to_string())),
            _ => Ok(None),
        }
    }

    pub(super) fn plan_ref(&mut self, t: TemplateId, scope: ElemId, m: NodeId) -> Result<PlanId> {
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
        // Right after `$s[k]` on a select: the next name is an interface name.
        let mut in_select: Option<ChoiceId> = None;
        // Past an interface name only props may follow.
        let mut after_iface = false;
        for (i, &part) in parts.iter().enumerate() {
            if i > 0 {
                let name = d.strings.get(names[i]);
                let choice_here = match cur_elem {
                    Some(e) if !after_iface => match self.c.templates[cur_t].elems[e].body {
                        Body::Choice(cid) if self.c.templates[cur_t].elems[e].kind.prop_index(name).is_none() => Some(cid),
                        _ => None,
                    },
                    _ => in_select.take(),
                };
                if let Some(cid) = choice_here {
                    if cur_elem.is_some() && !self.c.choices[cid].reactive {
                        // `$s.x` is `$s[1].x` when the select picks one option.
                        if self.c.choices[cid].num_to_select != 1 {
                            return Err(Error::Banned(format!("'${display}' needs an index: the select picks {} options", self.c.choices[cid].num_to_select)));
                        }
                        steps.push(Step::Index(IndexPlan { terms: vec![IndexTerm::Const(1)] }));
                    }
                    let (step, tpl, x) = self.iface_step(cid, name, &display)?;
                    steps.push(step);
                    cur_t = tpl;
                    cur_elem = Some(x);
                    after_iface = true;
                    if d.part_indices(part).next().is_some() {
                        return Err(Error::Banned(format!("'${display}': an index after an interface name")));
                    }
                    continue;
                }
                match cur_elem {
                    None => {
                        let e = self.child_named(cur_t, ROOT_SCOPE, name)?.ok_or_else(|| Error::UnknownName(display.clone()))?;
                        steps.push(Step::Elem(e));
                        cur_elem = Some(e);
                    }
                    // A descendant of the component: `$g.p`.
                    Some(e) if !after_iface && self.child_named(cur_t, e, name)?.is_some() => {
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
                        let name = kind.canonical_prop(name);
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
                    if in_select.is_some() {
                        return Err(Error::Banned(format!("'${display}' reaches into a select's option by position; name the content and use $s[k].name")));
                    }
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
                    Body::Choice(cid) if !self.c.choices[cid].reactive && !after_iface => {
                        let ip = self.plan_index(t, expr, &display)?;
                        steps.push(Step::Index(ip));
                        cur_elem = None;
                        in_select = Some(cid);
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
        // A bare `$s` names the one option a select picks.
        if let (Some(e), None, false) = (cur_elem, &prop, after_iface)
            && let Body::Choice(cid) = self.c.templates[cur_t].elems[e].body
            && !self.c.choices[cid].reactive
        {
            if self.c.choices[cid].num_to_select != 1 {
                return Err(Error::Banned(format!("'${display}' needs an index: the select picks {} options", self.c.choices[cid].num_to_select)));
            }
            steps.push(Step::Index(IndexPlan { terms: vec![IndexTerm::Const(1)] }));
        }
        self.c.plans.push(RefPlan { hops, steps, prop, display });
        Ok(self.c.plans.len() - 1)
    }

    /// A literal integer index (array props are static, so `[$n]` is not
    /// supported on them).
    pub(super) fn literal_index(&self, expr: &[NodeId], display: &str) -> Result<i64> {
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
    pub(super) fn plan_index(&self, t: TemplateId, expr: &[NodeId], display: &str) -> Result<IndexPlan> {
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

    pub(super) fn elem_label(&self, t: TemplateId, e: ElemId) -> String {
        let el = &self.c.templates[t].elems[e];
        if el.name != NONE { self.c.dast.strings.get(el.name).trim().to_string() } else { format!("<{}>", el.kind.tag()) }
    }

    pub(super) fn attr_text(&self, a: u32) -> Option<String> {
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

    pub(super) fn single_macro(&self, a: u32) -> Option<NodeId> {
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

    pub(super) fn is_blank(&self, n: NodeId) -> bool {
        match self.c.dast.kind(n) {
            NodeKind::Text => self.c.dast.str(n).trim().is_empty(),
            NodeKind::Other => true,
            _ => false,
        }
    }
}
