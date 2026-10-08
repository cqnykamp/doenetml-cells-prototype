//! Expand: stamp templates into components per scope, give `$ref` children
//! their kind, expand collects and point lists, and turn every plan into a
//! source (an essential value, an alias, or an operator over slots).

use super::*;

impl<'c, 'a> Builder<'c, 'a> {
    pub(super) fn new(c: &'c Compiled<'a>, prior: &'c Prior, engine: &'c mut dyn SymEngine) -> Self {
        // Size the columns from the previous build when there was one.
        let guess = prior.values.iter().map(|v| v.len()).sum::<usize>().max(c.templates.iter().map(|t| t.elems.len()).sum::<usize>() * 2);
        Builder {
            c,
            prior,
            engine,
            sym_templates: Vec::new(),
            math_slots: Vec::new(),
            symbolic: Vec::new(),
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
            choice_insts: Vec::new(),
            comp_choice: HashMap::new(),
            missing: None,
            arena: Arena::default(),
            root: 0,
        }
    }

    // ---- components and slots ------------------------------------------------

    /// `n_slots` is the public prop count plus any hidden slots the
    /// element's plan added.
    pub(super) fn new_component(&mut self, kind: ComponentKind, name: StrId, parent: CompIdx, node: NodeId, scope: ScopeId, n_slots: usize) -> CompIdx {
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

    pub(super) fn allocate_slots(&mut self, comp: CompIdx, n: usize) {
        self.slot_base[comp as usize] = self.sources.len() as u32;
        self.sources.extend(std::iter::repeat_n(Source::Unset, n));
        self.slot_comp.extend(std::iter::repeat_n(comp, n));
    }

    #[inline]
    pub(super) fn slot(&self, comp: CompIdx, prop_index: usize) -> SlotId {
        self.slot_base[comp as usize] + prop_index as u32
    }

    pub(super) fn set_children(&mut self, comp: CompIdx, kids: &[u32]) {
        self.comps.child_start[comp as usize] = self.comps.child_list.len() as u32;
        self.comps.child_count[comp as usize] = kids.len() as u32;
        self.comps.child_list.extend_from_slice(kids);
    }

    /// A slot that belongs to no component: a lowered math subexpression, a
    /// literal inside one, the missing-referent cell.
    pub(super) fn anon_slot(&mut self, source: Source) -> SlotId {
        let s = self.sources.len() as SlotId;
        self.sources.push(source);
        self.slot_comp.push(NONE);
        s
    }

    pub(super) fn missing_slot(&mut self) -> SlotId {
        if let Some(s) = self.missing {
            return s;
        }
        let s = self.anon_slot(Source::Fixed(f64::NAN));
        self.missing = Some(s);
        s
    }

    pub(super) fn comp_label(&self, comp: CompIdx) -> String {
        let name = self.comps.name[comp as usize];
        if name != NONE { self.c.dast.strings.get(name).trim().to_string() } else { format!("<{}>#{}", self.comps.kind[comp as usize].tag(), comp) }
    }

    pub(super) fn slot_label(&self, slot: SlotId) -> String {
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
    pub(super) fn scope_for(&mut self, parent: ScopeId, node: NodeId, k: u32) -> ScopeId {
        if let Some(&s) = self.scope_index.get(&(parent, node, k)) {
            return s;
        }
        self.scopes.push((parent, node, k));
        let s = (self.scopes.len() - 1) as ScopeId;
        self.scope_index.insert((parent, node, k), s);
        s
    }

    pub(super) fn enter_scope(&mut self, scope: ScopeId, template: TemplateId) {
        let s = scope as usize;
        if self.scope_comps.len() <= s {
            self.scope_comps.resize(s + 1, Vec::new());
        }
        self.scope_comps[s] = vec![NONE; self.c.templates[template].elems.len()];
    }

    // ---- expansion -----------------------------------------------------------

    pub(super) fn expand_all(&mut self) -> Result<()> {
        self.enter_scope(0, 0);
        let kids = self.expand(0, 0, NONE)?;
        self.root = kids.iter().copied().find(|&k| k & TEXT_BIT == 0).expect("document root");
        Ok(())
    }

    /// Instantiate template `t` in `scope`; returns the child entries of the
    /// template's own children (the repeat body, or the document).
    pub(super) fn expand(&mut self, t: TemplateId, scope: ScopeId, parent: CompIdx) -> Result<Vec<u32>> {
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
                Body::Choice(cid) => {
                    let node = self.c.templates[t].elems[e].node;
                    self.expand_choice(cid, node, scope, comp)?
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

    pub(super) fn child_entries(&mut self, t: TemplateId, e: ElemId, scope: ScopeId, parent: CompIdx) -> Vec<u32> {
        let n = self.c.templates[t].elems[e].children.len();
        let mut kids = Vec::with_capacity(n);
        for i in 0..n {
            let ch = &self.c.templates[t].elems[e].children[i];
            kids.push(self.child_entry(ch, scope, parent));
        }
        kids
    }

    pub(super) fn child_entry(&mut self, ch: &Child, scope: ScopeId, parent: CompIdx) -> u32 {
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
    pub(super) fn resolve_all(&mut self) -> Result<()> {
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
        self.choice_sources();
        for i in 0..self.instances.len() {
            let inst = self.instances[i];
            self.instance_sources(inst)?;
        }
        Ok(())
    }

    /// Give a `$ref` child its kind: a copy of a component, a number aliasing
    /// one prop, or a number holding the missing-referent cell.
    pub(super) fn place(&mut self, idx: CompIdx, plan: PlanId, scope: ScopeId) -> Result<()> {
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
    pub(super) fn copy_into(&mut self, idx: CompIdx, referent: CompIdx) {
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
    pub(super) fn resolve_items(&mut self, plan: PlanId, scope: ScopeId) -> Result<Vec<[SlotId; 2]>> {
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
    pub(super) fn expand_pointlist(&mut self, comp: CompIdx) -> Result<()> {
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

    pub(super) fn expand_collect(&mut self, comp: CompIdx) -> Result<()> {
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
    pub(super) fn collect_descendants(&self, c: CompIdx, kind: ComponentKind, out: &mut Vec<CompIdx>) {
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
    pub(super) fn iteration_components(&self, repeat: CompIdx, scope: ScopeId) -> Vec<CompIdx> {
        let (s, n) = (self.comps.child_start[repeat as usize] as usize, self.comps.child_count[repeat as usize] as usize);
        self.comps.child_list[s..s + n].iter().copied().filter(|&e| e & TEXT_BIT == 0 && self.comps.scope[e as usize] == scope).collect()
    }

    /// Sources for every prop of one instantiated element.
    pub(super) fn instance_sources(&mut self, inst: Instance) -> Result<()> {
        let comp = inst.comp;
        let (kind, extend_plan) = {
            let el = &self.c.templates[inst.template].elems[inst.elem];
            (el.kind, el.extend)
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
                SourcePlan::Default(v) => Source::Default(*v),
                SourcePlan::Inherit => Source::Alias(self.slot(extend.expect("Inherit needs an extend referent"), pi)),
                SourcePlan::InheritFrom(k) => Source::Alias(self.slot(extend.expect("Inherit needs an extend referent"), *k as usize)),
                SourcePlan::Fixed(v) => Source::Fixed(*v),
                SourcePlan::Alias(arg) => Source::Alias(self.arg_slot(*arg, comp, inst.scope, kind, pi)?),
                SourcePlan::Op(spec, args) => {
                    let start = self.op_inputs.len() as u32;
                    for &a in args {
                        let slot = self.arg_slot(a, comp, inst.scope, kind, pi)?;
                        self.op_inputs.push(slot);
                    }
                    Source::Op(*spec, start, args.len() as u8)
                }
                SourcePlan::IterIndex => Source::Fixed(self.scopes[inst.scope as usize].2 as f64),
                SourcePlan::Math(expr) => {
                    let id = self.instantiate_expr(*expr, inst.scope)?;
                    if self.arena.is_numeric(id) { Source::Alias(self.lower(id)) } else { Source::Fixed(f64::NAN) }
                }
                SourcePlan::MathHandle(expr, post) => {
                    if self.is_symbolic(comp) {
                        self.sym_source(*expr, *post, inst.scope)?
                    } else {
                        // A numeric math is not a math cell (ADR 0005).
                        Source::Fixed(f64::NAN)
                    }
                }
                SourcePlan::MathValue(expr) => {
                    if self.is_symbolic(comp) {
                        let start = self.op_inputs.len() as u32;
                        self.op_inputs.push(self.slot(comp, 0));
                        Source::Op(OpSpec::Sym(SymKind::Evaluate), start, 1)
                    } else {
                        let id = self.instantiate_expr(*expr, inst.scope)?;
                        if let Expr::Num(v) = *self.arena.get(id) {
                            // `<math>5</math>` is state, as a number literal is: a
                            // drag that reaches it changes it, as in the current core.
                            Source::Literal(v)
                        } else {
                            Source::Alias(self.lower(id))
                        }
                    }
                }
                SourcePlan::SymExpr(expr, post) => self.sym_source(*expr, *post, inst.scope)?,
                SourcePlan::MathEssential(tree) => {
                    self.math_slots.push(s);
                    match tree {
                        Some(tree) => Source::Literal(self.engine.import(tree) as f64),
                        None => Source::Literal(f64::NAN),
                    }
                }
                SourcePlan::Vec(op, args) => {
                    let start = self.op_inputs.len() as u32;
                    for &a in args {
                        let slot = self.arg_slot(a, comp, inst.scope, kind, pi)?;
                        self.op_inputs.push(slot);
                    }
                    Source::Vec(*op, start, args.len() as u8)
                }
                SourcePlan::VecOut(head, k) => Source::VecOut(self.slot(comp, *head as usize), *k),
            };
            if let (Source::Fixed(h), SourcePlan::MathHandle(..) | SourcePlan::SymExpr(..)) = (&source, &c.templates[inst.template].elems[inst.elem].props[pi])
                && !h.is_nan()
            {
                self.math_slots.push(s);
            }
            self.sources[s as usize] = source;
        }
        Ok(())
    }

    // ---- symbolic math ---------------------------------------------------------

    /// A math cell from a template's math text: a fixed handle when it has
    /// no cell leaves, an alias when it is one math leaf and nothing else,
    /// else an `Instantiate` over its leaves (each a number or a math cell).
    fn sym_source(&mut self, expr: ExprId, post: Post, scope: ScopeId) -> Result<Source> {
        let text = self.c.sym_text.get(&expr).ok_or_else(|| Error::BadMath { text: format!("{:?}", self.c.arena.get(expr)), reason: "no math text recorded".into() })?;
        let tree = cells_sym::parse::parse(text).map_err(|reason| Error::BadMath { text: text.clone(), reason })?;
        let mut leaves = Vec::new();
        let tree = self.bind_leaves(&tree, scope, &mut leaves)?;
        if leaves.is_empty() {
            let h = self.engine.import(&tree);
            let h = match post {
                Post::None => h,
                Post::Simplify => self.engine.simplify(h),
                Post::Expand => self.engine.expand(h),
            };
            return Ok(Source::Fixed(h as f64));
        }
        if let (Tree::Cell { cell, math: true }, Post::None) = (&tree, post) {
            return Ok(Source::Alias(*cell));
        }
        let template = self.sym_templates.len() as u32;
        self.sym_templates.push(tree);
        let start = self.op_inputs.len() as u32;
        let n = u8::try_from(leaves.len()).map_err(|_| Error::BadMath { text: text.clone(), reason: "more than 255 references".into() })?;
        self.op_inputs.extend_from_slice(&leaves);
        Ok(Source::Op(OpSpec::Sym(SymKind::Instantiate { template, post }), start, n))
    }

    /// Rebind a parsed template's `#plan` leaves to slots: a math leaf where
    /// the reference names an expression, else a numeric leaf. Distinct leaf
    /// slots are appended to `leaves`.
    fn bind_leaves(&mut self, t: &Tree, scope: ScopeId, leaves: &mut Vec<SlotId>) -> Result<Tree> {
        let mut kids = |ts: &[Tree], this: &mut Self| ts.iter().map(|k| this.bind_leaves(k, scope, leaves)).collect::<Result<Vec<_>>>();
        Ok(match t {
            Tree::Cell { cell: plan, .. } => {
                let (slot, math) = self.leaf_slot(*plan as PlanId, scope)?;
                if !leaves.contains(&slot) {
                    leaves.push(slot);
                }
                Tree::Cell { cell: slot, math }
            }
            Tree::Num(_) | Tree::Sym(_) => t.clone(),
            Tree::Add(ts) => Tree::Add(kids(ts, self)?),
            Tree::Mul(ts) => Tree::Mul(kids(ts, self)?),
            Tree::Sub(a, b) => Tree::Sub(Box::new(self.bind_leaves(a, scope, leaves)?), Box::new(self.bind_leaves(b, scope, leaves)?)),
            Tree::Div(a, b) => Tree::Div(Box::new(self.bind_leaves(a, scope, leaves)?), Box::new(self.bind_leaves(b, scope, leaves)?)),
            Tree::Pow(a, b) => Tree::Pow(Box::new(self.bind_leaves(a, scope, leaves)?), Box::new(self.bind_leaves(b, scope, leaves)?)),
            Tree::Neg(a) => Tree::Neg(Box::new(self.bind_leaves(a, scope, leaves)?)),
            Tree::Apply(f, a) => Tree::Apply(f.clone(), Box::new(self.bind_leaves(a, scope, leaves)?)),
        })
    }

    /// The slot a `$ref` inside math names, and whether it holds an
    /// expression (then the slot is the referent's math cell).
    fn leaf_slot(&mut self, plan: PlanId, scope: ScopeId) -> Result<(SlotId, bool)> {
        let display = &self.c.plans[plan].display;
        let slot = self.resolve_one(plan, scope).map_err(|e| match e {
            Error::ArityMismatch { .. } => Error::BadMath { text: display.clone(), reason: "a reference inside math must name one cell".into() },
            other => other,
        })?;
        Ok(match self.math_target(slot) {
            Some(m) => (m, true),
            None => (slot, false),
        })
    }

    /// When `slot` is a prop that stands for an expression (a symbolic
    /// math's `expr` or `value`, a function, an answer's math props), the
    /// slot of that expression.
    fn math_target(&mut self, slot: SlotId) -> Option<SlotId> {
        let comp = self.slot_comp[slot as usize];
        if comp == NONE {
            return None;
        }
        let kind = self.comps.kind[comp as usize];
        let pi = (slot - self.slot_base[comp as usize]) as usize;
        let name = kind.prop_defs().get(pi)?.name;
        match (kind, name) {
            (ComponentKind::Answer, "response" | "correct" | "submitted") => Some(slot),
            (ComponentKind::Math | ComponentKind::MathInput | ComponentKind::Function | ComponentKind::Derivative, "expr" | "value") => {
                self.is_symbolic(comp).then(|| self.slot(comp, kind.prop_index("expr").unwrap()))
            }
            _ => None,
        }
    }

    /// Whether a component's `expr` is a math cell: a function, an unbound
    /// mathInput, a math with a free symbol or a reference to an
    /// expression. A copy is symbolic when its referent is. A reference
    /// cycle counts as numeric (the cycle is reported later).
    pub(super) fn is_symbolic(&mut self, comp: CompIdx) -> bool {
        if self.symbolic.len() < self.comps.len() {
            self.symbolic.resize(self.comps.len(), 0);
        }
        match self.symbolic[comp as usize] {
            1 | 3 => return false,
            2 => return true,
            _ => {}
        }
        self.symbolic[comp as usize] = 3;
        let kind = self.comps.kind[comp as usize];
        let inst = self.comp_instance[comp as usize];
        let yes = match kind {
            ComponentKind::Function | ComponentKind::Derivative => true,
            ComponentKind::Math | ComponentKind::MathInput if inst == NONE => match self.sources.get(self.slot(comp, 0) as usize) {
                Some(Source::Alias(t)) => {
                    let referent = self.slot_comp[*t as usize];
                    referent != NONE && self.is_symbolic(referent)
                }
                _ => false,
            },
            ComponentKind::MathInput => {
                let i = self.instances[inst as usize];
                matches!(self.c.templates[i.template].elems[i.elem].props.get(1), Some(SourcePlan::MathEssential(_)))
            }
            ComponentKind::Math => {
                let i = self.instances[inst as usize];
                match self.c.templates[i.template].elems[i.elem].props.first() {
                    Some(SourcePlan::MathHandle(id, _)) => {
                        let id = *id;
                        let mut syms = Vec::new();
                        self.c.arena.symbols(id, &mut syms);
                        let mut leaves = Vec::new();
                        self.c.arena.cell_leaves(id, &mut leaves);
                        !syms.is_empty() || leaves.into_iter().any(|p| self.resolve_one(p as PlanId, i.scope).is_ok_and(|s| self.math_target(s).is_some()))
                    }
                    _ => false,
                }
            }
            _ => false,
        };
        self.symbolic[comp as usize] = if yes { 2 } else { 1 };
        yes
    }

    /// The slot an `Arg` names for component `comp` in `scope`.
    pub(super) fn arg_slot(&mut self, arg: Arg, comp: CompIdx, scope: ScopeId, kind: ComponentKind, pi: usize) -> Result<SlotId> {
        Ok(match arg {
            Arg::Own(a) => self.slot(comp, a as usize),
            Arg::Elem(el, slot) => {
                let other = self.scope_comps[scope as usize][el];
                self.slot(other, slot as usize)
            }
            Arg::Ref(p, Sel::Whole) => self.resolve_one(p, scope).map_err(|e| arity_error(e, kind, kind.prop_defs().get(pi).map(|d| d.name).unwrap_or("args")))?,
            Arg::Ref(p, Sel::Coord(j)) => {
                let targets = self.resolve_ref(p, scope, Some(2))?;
                if targets.len() != 2 {
                    return Err(Error::ArityMismatch { kind: kind.tag().into(), prop: "coords".into(), expected: 2, got: targets.len() });
                }
                targets[j as usize]
            }
            Arg::Ref(p, Sel::Item(i, j)) => {
                let items = self.resolve_items(p, scope)?;
                match items.get(i as usize) {
                    Some(item) => item[j as usize],
                    None => self.missing_slot(),
                }
            }
        })
    }

    /// Copy an expression template into the document's arena with its plan
    /// leaves resolved to slots in `scope`.
    pub(super) fn instantiate_expr(&mut self, id: ExprId, scope: ScopeId) -> Result<ExprId> {
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
    pub(super) fn lower(&mut self, id: ExprId) -> SlotId {
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

    pub(super) fn op_slot(&mut self, spec: OpSpec, inputs: &[SlotId]) -> SlotId {
        let start = self.op_inputs.len() as u32;
        self.op_inputs.extend_from_slice(inputs);
        self.anon_slot(Source::Op(spec, start, inputs.len() as u8))
    }

    // ---- reference resolution at expansion time ------------------------------

    /// Walk a plan from `scope`. Returns where it arrived and the prop it
    /// named, if any. Every step is an array read.
    pub(super) fn resolve(&self, plan: PlanId, scope: ScopeId) -> Result<(Resolved, Option<&'c str>)> {
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
                Step::Iface(cid, u) => {
                    let (next, s) = self.resolve_iface(*cid, *u, cur);
                    cur = next;
                    sc = s;
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
                            ComponentKind::Select => {
                                let inst = &self.choice_insts[self.comp_choice[&c]];
                                if k < 1 || k as usize > inst.scopes.len() { Resolved::Missing } else { Resolved::Iter(c, inst.scopes[(k - 1) as usize]) }
                            }
                            _ => return Err(Error::NotIndexable(p.display.clone())),
                        },
                    };
                }
            }
        }
        Ok((cur, p.prop.as_deref()))
    }

    pub(super) fn index_value(&self, ip: &IndexPlan, scope: ScopeId) -> i64 {
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
    pub(super) fn single_component(&self, target: Resolved, plan: PlanId) -> Result<CompIdx> {
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
    pub(super) fn targets_of(&mut self, target: Resolved, prop: Option<&str>, plan: PlanId, expected: Option<usize>) -> Result<Vec<SlotId>> {
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

    pub(super) fn resolve_ref(&mut self, plan: PlanId, scope: ScopeId, expected: Option<usize>) -> Result<Vec<SlotId>> {
        let (target, prop) = self.resolve(plan, scope)?;
        self.targets_of(target, prop, plan, expected)
    }

    /// The one slot a reference names, without allocating. Errors with a
    /// placeholder `ArityMismatch` (callers fill in kind and prop) when the
    /// reference names several cells.
    pub(super) fn resolve_one(&mut self, plan: PlanId, scope: ScopeId) -> Result<SlotId> {
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
}
