//! Planned kinds: point, circle, line, line segment and polygon. Each
//! planner reads the element's attributes and children and chooses the
//! operator chain that produces the kind's public props (plan 3). The
//! inverse rules of the operators it uses are in `geo.rs`.

use super::*;

impl<'a> Compiler<'a> {
    /// `<point>`: coordinates from `coords`, from `x`/`y`, or from `(a, b)`
    /// children; constraint children wrap them in a projection.
    pub(super) fn plan_point(&mut self, t: TemplateId, e: ElemId, base: Option<NodeId>) -> Result<ElemPlan> {
        let d = self.c.dast;
        let (el, scope) = {
            let x = &self.c.templates[t].elems[e];
            (x.node, x.name_scope)
        };
        let mut ch = ElemPlan::new(ComponentKind::Point.prop_defs().len());
        let math_children = |me: &Self, node: NodeId| -> Vec<NodeId> { d.children(node).iter().copied().filter(|&n| !me.is_blank(n) && d.kind(n) != NodeKind::Element).collect() };
        let own_children = math_children(self, el);
        let base_children = base.map(|b| math_children(self, b)).unwrap_or_default();
        if let Some(a) = self.attr_or_inherited(el, base, "coords") {
            let xy = match self.single_macro(a) {
                Some(m) => {
                    let p = self.plan_ref(t, scope, m)?;
                    [SourcePlan::coord(p, 0), SourcePlan::coord(p, 1)]
                }
                None => {
                    let xy = self.plan_tuple(t, scope, d.attr_children(a))?;
                    [xy[0].clone(), xy[1].clone()]
                }
            };
            Self::set_point_with_roles(&mut ch, [0, 1], &PointPlan::Tuple(xy), ["x", "y"], "coords");
        } else if self.attr_or_inherited(el, base, "x").is_some() || self.attr_or_inherited(el, base, "y").is_some() || (own_children.is_empty() && base_children.is_empty()) {
            for (i, name) in ["x", "y"].into_iter().enumerate() {
                match self.attr_or_inherited(el, base, name) {
                    Some(a) => {
                        let plan = self.plan_scalar(t, scope, name, d.attr_children(a))?;
                        if let SourcePlan::Literal(v) = plan {
                            ch.from_attr(name, name);
                            ch.set_essential(i, name, v);
                        } else {
                            ch.set(i, plan);
                        }
                    }
                    None => ch.set_essential(i, name, 0.0),
                }
            }
        } else {
            let nodes = if own_children.is_empty() { base_children } else { own_children };
            let xy = self.plan_tuple(t, scope, &nodes)?;
            Self::set_point_with_roles(&mut ch, [0, 1], &PointPlan::Tuple([xy[0].clone(), xy[1].clone()]), ["x", "y"], "children");
        }
        // `hide` is a 0/1 cell: a bare or literal flag, or a bound cell; a
        // copy inherits the original's.
        match self.attr_or_inherited(el, base, "hide") {
            Some(a) if self.single_macro(a).is_some() => {
                let plan = self.plan_scalar(t, scope, "hide", d.attr_children(a))?;
                ch.set(2, plan);
            }
            Some(_) => {
                let on = self.attr_flag(el, "hide") || base.is_some_and(|b| self.attr_flag(b, "hide"));
                ch.set(2, SourcePlan::Literal(if on { 1.0 } else { 0.0 }));
            }
            None => ch.set(2, if base.is_some() { SourcePlan::Inherit } else { SourcePlan::Default(0.0) }),
        }
        // Constraints may sit directly under the point or in <constraints>.
        let mut constraints: Vec<NodeId> = Vec::new();
        for node in std::iter::once(el).chain(base) {
            for &n in d.children(node) {
                if d.kind(n) == NodeKind::Element {
                    if d.str(n) == "constraints" {
                        constraints.extend(d.children(n).iter().copied().filter(|&c| d.kind(c) == NodeKind::Element));
                    } else {
                        constraints.push(n);
                    }
                }
            }
            if !constraints.is_empty() {
                break;
            }
        }
        if let Some(&c) = constraints.first() {
            if constraints.len() > 1 {
                return Err(Error::Unsupported("more than one constraint on a point".into()));
            }
            self.wrap_constraint(t, scope, &mut ch, c)?;
        }
        Ok(ch)
    }

    /// Move the planned coordinates of a point to hidden raw slots and make
    /// the public `x`, `y` their projection onto the constraint.
    pub(super) fn wrap_constraint(&mut self, t: TemplateId, scope: ElemId, ch: &mut ElemPlan, c: NodeId) -> Result<()> {
        let d = self.c.dast;
        let (px, py) = (ch.props[0].take().expect("x planned"), ch.props[1].take().expect("y planned"));
        let raw_x = ch.hidden(px);
        let raw_y = ch.hidden(py);
        // The raw coordinates are the essential state now; keep their roles.
        for role in ["x", "y"] {
            if let Some(slot) = ch.roles.get_mut(role) {
                *slot = if role == "x" { raw_x } else { raw_y };
            }
        }
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
                let snap = |ch: &mut ElemPlan, raw: u8, step: f64, offset: f64| -> SourcePlan {
                    let mut cur = raw;
                    if offset != 0.0 {
                        cur = ch.hidden(SourcePlan::computed(OpSpec::Offset { k: -offset }, vec![cur]));
                    }
                    if step != 1.0 {
                        cur = ch.hidden(SourcePlan::computed(OpSpec::Scale { k: 1.0 / step }, vec![cur]));
                    }
                    cur = ch.hidden(SourcePlan::computed(OpSpec::Round, vec![cur]));
                    if step != 1.0 {
                        cur = ch.hidden(SourcePlan::computed(OpSpec::Scale { k: step }, vec![cur]));
                    }
                    if offset != 0.0 {
                        return SourcePlan::computed(OpSpec::Offset { k: offset }, vec![cur]);
                    }
                    SourcePlan::own(cur)
                };
                let px = snap(ch, raw_x, dx, xo);
                let py = snap(ch, raw_y, dy, yo);
                ch.set(0, px);
                ch.set(1, py);
            }
            "constrainTo" => {
                let m = d.children(c).iter().copied().find(|&n| d.kind(n) == NodeKind::Macro).ok_or_else(|| Error::Unsupported("<constrainTo> without a reference".into()))?;
                let p = self.plan_ref(t, scope, m)?;
                let target = self.plan_elem_target(t, p).ok_or_else(|| Error::Unsupported("<constrainTo> must name a component in the same scope".into()))?;
                let refs = |me: &mut Self, ch: &mut ElemPlan, props: &[&str]| -> Vec<u8> { props.iter().map(|pr| ch.hidden(SourcePlan::reference(me.plan_with_prop(p, pr)))).collect() };
                match self.c.templates[t].elems[target].kind {
                    ComponentKind::Circle => {
                        let c = refs(self, ch, &["cx", "cy", "radius"]);
                        ch.set(0, SourcePlan::vector(VecOp::ProjectCircle, vec![raw_x, raw_y, c[0], c[1], c[2]]));
                        ch.set(1, SourcePlan::VecOut(0, 1));
                    }
                    ComponentKind::Line | ComponentKind::LineSegment => {
                        let l = refs(self, ch, &["x1", "y1", "x2", "y2"]);
                        ch.set(0, SourcePlan::vector(VecOp::ProjectLine, vec![raw_x, raw_y, l[0], l[1], l[2], l[3]]));
                        ch.set(1, SourcePlan::VecOut(0, 1));
                    }
                    other => return Err(Error::Unsupported(format!("constrainTo a <{}>", other.tag()))),
                }
            }
            other => return Err(Error::Unsupported(format!("<{other}> constraint"))),
        }
        Ok(())
    }

    /// Dispatch for the geometric kinds.
    pub(super) fn plan_geo(&mut self, t: TemplateId, e: ElemId, extend: Option<PlanId>) -> Result<()> {
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
            self.c.templates[t].elems[e].props = vec![SourcePlan::Inherit; n];
            return Ok(());
        }
        // Merged attributes: the referent's node supplies what the copy omits.
        let base_elem = extend.and_then(|p| self.plan_elem_target(t, p)).filter(|&r| self.c.templates[t].elems[r].node != NONE);
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
                if ch.role_attr.get(role).is_some_and(|a| own_attrs.iter().any(|o| o == a)) {
                    continue;
                }
                if let Some(&bs) = base_roles.get(role)
                    && matches!(ch.props[slot as usize], Some(SourcePlan::Literal(_)))
                {
                    ch.props[slot as usize] = Some(SourcePlan::Alias(Arg::Elem(r, bs)));
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

    /// Attribute of the element, or of the `extend` referent when the
    /// element does not give it (a copy with overrides).
    pub(super) fn attr_or_inherited(&self, el: NodeId, base: Option<NodeId>, name: &str) -> Option<u32> {
        let d = self.c.dast;
        d.attr(el, name).or_else(|| base.and_then(|b| d.attr(b, name)))
    }

    /// Two hidden slots holding a point. A literal coordinate is essential
    /// state and gets a role from `roles`, so a copy with overrides shares it.
    pub(super) fn point_slots(ch: &mut ElemPlan, p: &PointPlan) -> [u8; 2] {
        [ch.hidden(p.coord(0)), ch.hidden(p.coord(1))]
    }

    /// Put a point's coordinates on two public slots, tagging literal
    /// coordinates with roles. A free shape's own points are the points
    /// themselves: no instruction stands between (ADR 0006).
    pub(super) fn set_point_with_roles(ch: &mut ElemPlan, slots: [usize; 2], p: &PointPlan, roles: [&'static str; 2], attr: &'static str) {
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

    pub(super) fn point_slots_with_roles(ch: &mut ElemPlan, p: &PointPlan, roles: [&'static str; 2], attr: &'static str) -> [u8; 2] {
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
    pub(super) fn plan_point_list(&mut self, t: TemplateId, scope: ElemId, a: u32) -> Result<Vec<PointPlan>> {
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
    pub(super) fn array_items_of_plan(&self, t: TemplateId, p: PlanId) -> Result<Option<usize>> {
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
    pub(super) fn count_points_in_attr(&self, el: NodeId, name: &str) -> Result<usize> {
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

    pub(super) fn plan_circle(&mut self, t: TemplateId, e: ElemId, base: Option<NodeId>) -> Result<ElemPlan> {
        let d = self.c.dast;
        let (el, scope) = {
            let x = &self.c.templates[t].elems[e];
            (x.node, x.name_scope)
        };
        let kind = ComponentKind::Circle;
        let center = match self.attr_or_inherited(el, base, "center") {
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
        let radius = match self.attr_or_inherited(el, base, "radius") {
            Some(a) => Some(self.plan_scalar(t, scope, "radius", d.attr_children(a))?),
            None => None,
        };
        let through = match self.attr_or_inherited(el, base, "through") {
            Some(a) => self.plan_point_list(t, scope, a)?,
            None => Vec::new(),
        };
        let n = through.len();
        let mut ch = ElemPlan::new(kind.prop_defs().len());
        // Computed public props (diameter, circumference) keep their defs;
        // area goes through a hidden square.
        for (i, def) in kind.prop_defs().iter().enumerate() {
            if let PropFrom::Computed { op, args } = def.from {
                ch.set(i, SourcePlan::computed(op, args.to_vec()));
            }
        let r2 = ch.hidden(SourcePlan::computed(OpSpec::Mul, vec![2, 2]));
        ch.set(5, SourcePlan::computed(OpSpec::Scale { k: std::f64::consts::PI }, vec![r2]));
        }
        let (hc, hr) = (center.is_some(), radius.is_some());
        let center_slots = |ch: &mut ElemPlan, c: &CenterPlan| -> [SourcePlan; 2] {
            match c {
                CenterPlan::Ref(p) => {
                    let _ = ch;
                    [SourcePlan::coord(*p, 0), SourcePlan::coord(*p, 1)]
                }
                CenterPlan::Tuple(v) => [v[0].clone(), v[1].clone()],
            }
        };
        // The radius shown is never negative; the prescribed or essential
        // radius behind it receives the clamped value (projection, ADR 0003).
        let clamped_radius = |ch: &mut ElemPlan, r: &SourcePlan| -> SourcePlan {
            let pres = match r {
                SourcePlan::Literal(v) => {
                    ch.from_attr("r", "radius");
                    ch.essential("r", *v)
                }
                other => ch.hidden(other.clone()),
            };
            SourcePlan::computed(OpSpec::Clamp { lo: 0.0, hi: f64::INFINITY }, vec![pres])
        };
        let essential_radius = |ch: &mut ElemPlan| -> SourcePlan {
            let pres = ch.essential("r", 1.0);
            SourcePlan::computed(OpSpec::Clamp { lo: 0.0, hi: f64::INFINITY }, vec![pres])
        };
        let nan = || SourcePlan::Fixed(f64::NAN);
        // Through points and the center-as-reference, for `$c.throughPoint1`
        // and `<point extend="$c.center">`.
        let through_slot = |i: usize, j: usize| kind.prop_index("throughX1").unwrap() + 2 * i + j;
        for (i, p) in through.iter().enumerate().take(3) {
            Self::set_point_with_roles(&mut ch, [through_slot(i, 0), through_slot(i, 1)], p, POINT_ROLES[i], "through");
        }
        for i in through.len()..3 {
            ch.set(through_slot(i, 0), nan());
            ch.set(through_slot(i, 1), nan());
        }
        ch.set(kind.prop_index("numThroughPoints").unwrap(), SourcePlan::Fixed(n as f64));
        let (center_x, center_y) = (kind.prop_index("centerX").unwrap(), kind.prop_index("centerY").unwrap());
        let (tx1, ty1, tx2, ty2) = (through_slot(0, 0) as u8, through_slot(0, 1) as u8, through_slot(1, 0) as u8, through_slot(1, 1) as u8);
        // The prescribed center lives on `centerX`/`centerY` and the cases
        // read it from there; without one they alias the derived center.
        match &center {
            Some(c) => {
                let [cx, cy] = center_slots(&mut ch, c);
                Self::set_point_with_roles(&mut ch, [center_x, center_y], &PointPlan::Tuple([cx, cy]), ["cx", "cy"], "center");
            }
            None => {
                ch.set(center_x, SourcePlan::own(0));
                ch.set(center_y, SourcePlan::own(1));
            }
        }
        let (center_x, center_y) = (center_x as u8, center_y as u8);
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
                ch.set(0, SourcePlan::own(center_x));
                ch.set(1, SourcePlan::own(center_y));
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
                ch.set(0, SourcePlan::own(center_x));
                ch.set(1, SourcePlan::own(center_y));
                let r = clamped_radius(&mut ch, radius.as_ref().unwrap());
                ch.set(2, r);
            }
            (true, false, 1) => {
                ch.set(0, SourcePlan::vector(VecOp::CircleCenterPoint, vec![center_x, center_y, tx1, ty1]));
                ch.set(1, SourcePlan::VecOut(0, 1));
                ch.set(2, SourcePlan::VecOut(0, 2));
            }
            (false, has_r, 1) => {
                // The through point sits on top of the circle.
                let r = if has_r { clamped_radius(&mut ch, radius.as_ref().unwrap()) } else { essential_radius(&mut ch) };
                ch.set(2, r);
                ch.set(0, SourcePlan::own(tx1));
                ch.set(1, SourcePlan::computed(OpSpec::Sub, vec![ty1, 2]));
            }
            (false, true, 2) => {
                let r = clamped_radius(&mut ch, radius.as_ref().unwrap());
                ch.set(2, r);
                ch.set(0, SourcePlan::vector(VecOp::CircleTwoPointsRadius, vec![tx1, ty1, tx2, ty2, 2]));
                ch.set(1, SourcePlan::VecOut(0, 1));
            }
            (false, false, n) => {
                let args: Vec<u8> = (0..2 * n as u8).map(|k| tx1 + k).collect();
                ch.set(0, SourcePlan::vector(VecOp::CirclePoints { n: n as u8 }, args));
                ch.set(1, SourcePlan::VecOut(0, 1));
                ch.set(2, SourcePlan::VecOut(0, 2));
            }
            _ => unreachable!(),
        }
        Ok(ch)
    }

    /// Replace cell leaves that name a `<math>` element of this template
    /// with that math's own expression when it has free symbols.
    pub(super) fn inline_symbolic_maths(&mut self, t: TemplateId, id: ExprId) -> ExprId {
        let e = self.c.arena.get(id).clone();
        match e {
            Expr::Cell(p) => {
                let plan = &self.c.plans[p as usize];
                let is_value = plan.prop.as_deref().is_none_or(|pr| pr == "value");
                let Some(target) = (is_value).then(|| self.plan_elem_target(t, p as PlanId)).flatten() else {
                    return id;
                };
                // The math may come later in the document and not be planned yet.
                if self.c.templates[t].elems[target].kind == ComponentKind::Math && self.c.templates[t].elems[target].props.is_empty() && self.plan_elem(t, target).is_err() {
                    return id;
                }
                match self.c.templates[t].elems[target].props.first() {
                    Some(SourcePlan::MathHandle(inner)) => {
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
    pub(super) fn line_variables(&self, el: NodeId, base: Option<NodeId>) -> (String, String) {
        match self.attr_or_inherited(el, base, "variables").and_then(|a| self.attr_text(a)) {
            Some(text) => {
                let names: Vec<String> = text.split(|c: char| !c.is_alphanumeric()).filter(|s| !s.is_empty()).map(str::to_string).collect();
                if names.len() == 2 { (names[0].clone(), names[1].clone()) } else { ("x".into(), "y".into()) }
            }
            None => ("x".into(), "y".into()),
        }
    }

    pub(super) fn plan_line(&mut self, t: TemplateId, e: ElemId, base: Option<NodeId>) -> Result<ElemPlan> {
        let d = self.c.dast;
        let (el, scope) = {
            let x = &self.c.templates[t].elems[e];
            (x.node, x.name_scope)
        };
        let kind = ComponentKind::Line;
        let mut ch = ElemPlan::new(kind.prop_defs().len());
        let through = match self.attr_or_inherited(el, base, "through") {
            Some(a) => self.plan_point_list(t, scope, a)?,
            None => Vec::new(),
        };
        let slope = self.attr_or_inherited(el, base, "slope");
        let parallel = self.attr_or_inherited(el, base, "parallelTo");
        let perpendicular = self.attr_or_inherited(el, base, "perpendicularTo");
        let equation_attr = self.attr_or_inherited(el, base, "equation");
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
            ch.set(0, SourcePlan::vector(VecOp::LinePointsFromCoeffs, vec![7, 8, 9]));
            ch.set(1, SourcePlan::VecOut(0, 1));
            ch.set(2, SourcePlan::VecOut(0, 2));
            ch.set(3, SourcePlan::VecOut(0, 3));
            let q = ch.hidden(SourcePlan::computed(OpSpec::Div, vec![7, 8]));
            ch.set(4, SourcePlan::computed(OpSpec::Negate, vec![q]));
            let xi = ch.hidden(SourcePlan::computed(OpSpec::Div, vec![9, 7]));
            ch.set(5, SourcePlan::computed(OpSpec::Negate, vec![xi]));
            let yi = ch.hidden(SourcePlan::computed(OpSpec::Div, vec![9, 8]));
            ch.set(6, SourcePlan::computed(OpSpec::Negate, vec![yi]));
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
                ch.set(2, SourcePlan::vector(VecOp::PolarSlope, vec![0, 1, m, dist]));
            } else {
                let (a, perp) = match (parallel, perpendicular) {
                    (Some(a), _) => (a, false),
                    (None, Some(a)) => (a, true),
                    _ => unreachable!(),
                };
                let [ux, uy] = self.plan_direction(t, scope, a, &mut ch)?;
                direction_slots = Some(if perp { [uy, ux] } else { [ux, uy] });
                ch.set(2, SourcePlan::vector(VecOp::PolarDirection { perpendicular: perp }, vec![0, 1, ux, uy, dist]));
            }
            ch.set(3, SourcePlan::VecOut(2, 1));
            ch.set(10, SourcePlan::Fixed(1.0));
        } else {
            // The line's points are the through points (or essential
            // defaults) themselves; a whole-line drag is a point group.
            let pt = |ch: &mut ElemPlan, slots: [usize; 2], p: Option<&PointPlan>, k: usize, roles: [&'static str; 2], default: [f64; 2]| match p {
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
        let dy = ch.hidden(SourcePlan::computed(OpSpec::Sub, vec![3, 1]));
        let dx = ch.hidden(SourcePlan::computed(OpSpec::Sub, vec![2, 0]));
        let _ = (slope_slot, direction_slots);
        ch.set(4, SourcePlan::computed(OpSpec::Div, vec![dy, dx]));
        let q = ch.hidden(SourcePlan::computed(OpSpec::Div, vec![1, 4]));
        ch.set(5, SourcePlan::computed(OpSpec::Sub, vec![0, q]));
        let mx = ch.hidden(SourcePlan::computed(OpSpec::Mul, vec![4, 0]));
        ch.set(6, SourcePlan::computed(OpSpec::Sub, vec![1, mx]));
        ch.set(7, SourcePlan::own(dy));
        ch.set(8, SourcePlan::computed(OpSpec::Sub, vec![0, 2]));
        let ax = ch.hidden(SourcePlan::computed(OpSpec::Mul, vec![7, 0]));
        let by = ch.hidden(SourcePlan::computed(OpSpec::Mul, vec![8, 1]));
        let sum = ch.hidden(SourcePlan::computed(OpSpec::Add, vec![ax, by]));
        ch.set(9, SourcePlan::computed(OpSpec::Negate, vec![sum]));
        Ok(ch)
    }

    /// Two hidden slots holding a direction: a point or tuple's coordinates,
    /// or another line's `point2 - point1`.
    pub(super) fn plan_direction(&mut self, t: TemplateId, scope: ElemId, a: u32, ch: &mut ElemPlan) -> Result<[u8; 2]> {
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
                        let ux = ch.hidden(SourcePlan::Op(OpSpec::Sub, vec![Arg::Ref(x2, Sel::Whole), Arg::Ref(x1, Sel::Whole)]));
                        let uy = ch.hidden(SourcePlan::Op(OpSpec::Sub, vec![Arg::Ref(y2, Sel::Whole), Arg::Ref(y1, Sel::Whole)]));
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

    pub(super) fn plan_segment(&mut self, t: TemplateId, e: ElemId, base: Option<NodeId>) -> Result<ElemPlan> {
        let (el, scope) = {
            let x = &self.c.templates[t].elems[e];
            (x.node, x.name_scope)
        };
        let mut ch = ElemPlan::new(ComponentKind::LineSegment.prop_defs().len());
        let ends = match self.attr_or_inherited(el, base, "endpoints") {
            Some(a) => self.plan_point_list(t, scope, a)?,
            None => Vec::new(),
        };
        let pt = |ch: &mut ElemPlan, slots: [usize; 2], p: Option<&PointPlan>, k: usize, roles: [&'static str; 2], default: [f64; 2]| match p {
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

    pub(super) fn plan_polygon(&mut self, t: TemplateId, e: ElemId, base: Option<NodeId>) -> Result<ElemPlan> {
        let d = self.c.dast;
        let (el, scope) = {
            let x = &self.c.templates[t].elems[e];
            (x.node, x.name_scope)
        };
        let kind = ComponentKind::Polygon;
        let mut ch = ElemPlan::new(kind.prop_defs().len());
        let vertices = match self.attr_or_inherited(el, base, "vertices") {
            Some(a) => self.plan_point_list(t, scope, a)?,
            None => Vec::new(),
        };
        let n = vertices.len();
        if n > crate::components::MAX_VERTICES {
            return Err(Error::Unsupported(format!("a polygon with more than {} vertices", crate::components::MAX_VERTICES)));
        }
        let flag = |me: &Self, name: &str, default: bool| -> bool {
            match me.attr_or_inherited(el, base, name) {
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
        let pivot_point = match self.attr_or_inherited(el, base, "rotationCenter") {
            Some(a) if flag(self, "rotateAround", false) || self.attr_or_inherited(el, base, "rotateAround").and_then(|r| self.attr_text(r)).is_some_and(|r| r.trim() == "point") => match self.single_macro(a) {
                Some(m) => Some(PointPlan::Ref(self.plan_ref(t, scope, m)?)),
                None => {
                    let xy = self.plan_tuple(t, scope, d.attr_children(a))?;
                    Some(PointPlan::Tuple([xy[0].clone(), xy[1].clone()]))
                }
            },
            _ => None,
        };
        let rigid_opts = if rigid || similar {
            let rotate_around = self.attr_or_inherited(el, base, "rotateAround").and_then(|a| self.attr_text(a)).unwrap_or_default();
            let pivot = match rotate_around.trim() {
                "vertex" => {
                    let k = self.attr_or_inherited(el, base, "rotationVertex").and_then(|a| self.attr_text(a)).and_then(|s| s.trim().parse::<usize>().ok()).unwrap_or(1);
                    Pivot::Vertex(k.saturating_sub(1) as u8)
                }
                "point" if pivot_point.is_some() => Pivot::Point,
                _ => Pivot::Centroid,
            };
            let min_shrink = self.attr_or_inherited(el, base, "minShrink").and_then(|a| self.attr_text(a)).and_then(|s| s.trim().parse::<f64>().ok()).unwrap_or(0.1);
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
                // The pivot point rides along as two more inputs, NaN when
                // the shape rotates about a centroid or vertex.
                match &pivot_point {
                    Some(pp) => {
                        let [x, y] = Self::point_slots(&mut ch, pp);
                        args.push(x);
                        args.push(y);
                    }
                    None => {
                        args.push(ch.hidden(SourcePlan::Fixed(f64::NAN)));
                        args.push(ch.hidden(SourcePlan::Fixed(f64::NAN)));
                    }
                }
                ch.set(1, SourcePlan::vector(VecOp::Shape { n: n as u8, opts }, args));
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
}
