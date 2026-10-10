//! Planned kinds: point, circle, line, line segment and polygon. Each
//! planner reads the element's attributes and children and chooses the
//! operator chain that produces the kind's public props (plan 3). The
//! inverse rules of the operators it uses are in `program/geo.rs`.
//!
//! This file holds the dispatcher (`plan_geo`) and what the kinds share:
//! point slots and roles, point lists, and inherited attributes. Each kind's
//! planner is in its own file.

use super::*;

mod circle;
mod line;
mod point;
mod polygon;

impl<'a> Compiler<'a> {
    /// Dispatch for the geometric kinds.
    pub(in crate::build) fn plan_geo(
        &mut self,
        t: TemplateId,
        e: ElemId,
        extend: Option<RefId>,
    ) -> Result<()> {
        let d = self.c.dast;
        let Elem { node: el, kind, .. } = self.c.templates[t].elems[e];
        // Own attributes other than name and extend, or children, override
        // the referent's; with none, the copy aliases every public prop.
        let own_attrs: Vec<String> = d
            .attrs(el)
            .map(|a| d.attr_name(a).to_string())
            .filter(|n| n != "name" && n != "extend")
            .collect();
        let has_children = !d.children(el).iter().all(|&n| self.is_blank(n));
        if extend.is_some() && own_attrs.is_empty() && !has_children {
            let n = kind.prop_defs().len();
            self.c.templates[t].elems[e].props = vec![SourcePlan::Inherit; n];
            return Ok(());
        }
        // Merged attributes: the referent's node supplies what the copy omits.
        let base_elem = extend
            .and_then(|p| self.plan_elem_target(t, p))
            .filter(|&r| self.c.templates[t].elems[r].node != NONE);
        let base = base_elem.map(|r| self.c.templates[t].elems[r].node);
        let mut ch = match kind {
            ComponentKind::Point => self.plan_point(t, e, base)?,
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
                if ch
                    .role_attr
                    .get(role)
                    .is_some_and(|a| own_attrs.iter().any(|o| o == a))
                {
                    continue;
                }
                if let Some(&bs) = base_roles.get(role)
                    && matches!(ch.props[slot as usize], Some(SourcePlan::Literal(_)))
                {
                    ch.props[slot as usize] = Some(SourcePlan::Alias(Arg::Elem(r, bs)));
                }
            }
        }
        let scope = self.c.templates[t].elems[e].name_scope;
        self.plan_fix(t, scope, el, &["fixed"])?
            .apply(&mut ch.props);
        self.c.templates[t].elems[e].roles = ch.roles.clone();
        self.c.templates[t].elems[e].props = ch.finish();
        Ok(())
    }

    /// Attribute of the element, or of the `extend` referent when the
    /// element does not give it (a copy with overrides).
    /// The children that make up math text: text and `$ref`s, not blanks
    /// or elements (constraints).
    fn math_children(&self, node: NodeId) -> Vec<NodeId> {
        let d = self.c.dast;
        d.children(node)
            .iter()
            .copied()
            .filter(|&n| !self.is_blank(n) && d.kind(n) != NodeKind::Element)
            .collect()
    }

    pub(in crate::build) fn attr_or_inherited(
        &self,
        el: NodeId,
        base: Option<NodeId>,
        name: &str,
    ) -> Option<u32> {
        let d = self.c.dast;
        d.attr(el, name)
            .or_else(|| base.and_then(|b| d.attr(b, name)))
    }

    /// Two hidden slots holding a point. A literal coordinate is essential
    /// state and gets a role from `roles`, so a copy with overrides shares it.
    pub(in crate::build) fn point_slots(ch: &mut ElemPlan, p: &PointPlan) -> [usize; 2] {
        [ch.hidden(p.coord(0)), ch.hidden(p.coord(1))]
    }

    /// Put a point's coordinates on two public slots, tagging literal
    /// coordinates with roles. A free shape's own points are the points
    /// themselves: no instruction stands between (ADR 0006).
    pub(in crate::build) fn set_point_with_roles(
        ch: &mut ElemPlan,
        slots: [usize; 2],
        p: &PointPlan,
        roles: [&'static str; 2],
        attr: &'static str,
    ) {
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

    /// A given point as `set_point_with_roles` sets it, else an essential
    /// default with its roles.
    fn set_point_or_default(
        ch: &mut ElemPlan,
        slots: [usize; 2],
        p: Option<&PointPlan>,
        roles: [&'static str; 2],
        attr: &'static str,
        (default_roles, default): DefaultPoint,
    ) {
        match p {
            Some(p) => Self::set_point_with_roles(ch, slots, p, roles, attr),
            None => {
                ch.set_essential(slots[0], default_roles[0], default[0]);
                ch.set_essential(slots[1], default_roles[1], default[1]);
            }
        }
    }

    pub(in crate::build) fn point_slots_with_roles(
        ch: &mut ElemPlan,
        p: &PointPlan,
        roles: [&'static str; 2],
        attr: &'static str,
    ) -> [usize; 2] {
        let mut out = [0; 2];
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
    pub(in crate::build) fn plan_point_list(
        &mut self,
        t: TemplateId,
        scope: ElemId,
        a: u32,
    ) -> Result<Vec<PointPlan>> {
        let d = self.c.dast;
        let (toks, text) = self.math_tokens(t, scope, d.attr_children(a))?;
        let mut out = Vec::new();
        let mut i = 0;
        while i < toks.len() {
            match &toks[i] {
                Token::Cell(p) => {
                    let p = *p as RefId;
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
                                return Err(Error::BadMath {
                                    text,
                                    reason: "missing ')'".into(),
                                });
                            }
                            _ => {}
                        }
                        i += 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    let inner = super::expr::unwrap_parens(&toks[start..i]).unwrap();
                    out.push(PointPlan::Tuple(self.tuple_from_tokens(inner, &text)?));
                }
                Token::Comma => i += 1,
                // A symbol (`through="A"`) is not a point here: NaN, as the
                // current core's warning case.
                Token::Ident(_) => {
                    out.push(PointPlan::Tuple([
                        SourcePlan::Fixed(f64::NAN),
                        SourcePlan::Fixed(f64::NAN),
                    ]));
                    i += 1;
                }
                other => {
                    return Err(Error::BadMath {
                        text,
                        reason: format!("unexpected {other:?} in a point list"),
                    });
                }
            }
        }
        Ok(out)
    }

    /// If plan `p` names an array prop, how many items it has (known at
    /// compile time from the referent element).
    pub(in crate::build) fn array_items_of_plan(
        &self,
        t: TemplateId,
        p: RefId,
    ) -> Result<Option<usize>> {
        let plan = &self.c.refs[p];
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
                Step::Index(_) | Step::Iface(..) => return Ok(None),
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
    pub(in crate::build) fn count_points_in_attr(&self, el: NodeId, name: &str) -> Result<usize> {
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

    /// Replace cell leaves that name a `<math>` element of this template
    /// with that math's own expression when it has free symbols.
    pub(in crate::build) fn inline_symbolic_maths(&mut self, t: TemplateId, id: ExprId) -> ExprId {
        let e = self.c.arena.get(id).clone();
        match e {
            Expr::Cell(p) => {
                let plan = &self.c.refs[p as usize];
                let is_value = plan.prop.as_deref().is_none_or(|pr| pr == "value");
                let Some(target) = (is_value)
                    .then(|| self.plan_elem_target(t, p as RefId))
                    .flatten()
                else {
                    return id;
                };
                // The math may come later in the document and not be planned yet.
                if self.c.templates[t].elems[target].kind == ComponentKind::Math
                    && self.c.templates[t].elems[target].props.is_empty()
                    && self.plan_elem(t, target).is_err()
                {
                    return id;
                }
                match self.c.templates[t].elems[target].props.first() {
                    Some(SourcePlan::MathHandle(inner, _)) => {
                        let inner = *inner;
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
            Expr::Add(a, b)
            | Expr::Sub(a, b)
            | Expr::Mul(a, b)
            | Expr::Div(a, b)
            | Expr::Pow(a, b) => {
                let (na, nb) = (
                    self.inline_symbolic_maths(t, a),
                    self.inline_symbolic_maths(t, b),
                );
                self.c.arena.push(e.with_operands(na, nb))
            }
        }
    }
}

/// A line's or segment's default point: its essential roles and value. The
/// current core takes a missing first point as (1, 0) and a missing second
/// one as (0, 0); a direction-based line's first point is the latter.
type DefaultPoint = ([&'static str; 2], [f64; 2]);
const ESS1: DefaultPoint = (["ess1x", "ess1y"], [1.0, 0.0]);
const ESS2: DefaultPoint = (["ess2x", "ess2y"], [0.0, 0.0]);
