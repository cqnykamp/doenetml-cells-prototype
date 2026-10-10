//! Expand: stamp templates into components per scope, give `$ref` children
//! their kind, expand collects and point lists, and turn every plan into a
//! source (an essential value, an alias, or an operator over slots).

use super::*;

// ---------------------------------------------------------------------------
// Expansion state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(super) enum Source {
    Unset,
    Literal(f64),
    Default(f64),
    /// A constant that is not essential: an iteration index, a collect's
    /// count, a math handle, the shared missing-referent cell.
    Fixed(f64),
    Alias(SlotId),
    /// Operator over `op_inputs[start..start + n]`; for a vector operator,
    /// the head (output 0).
    Op(OpSpec, u32, u8),
    /// Output `k` of the vector instruction headed at `head`.
    VecOut(SlotId, u8),
}

/// `Builder::is_symbolic`'s memo for one component.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(super) enum MathMode {
    #[default]
    Unknown,
    Numeric,
    Symbolic,
    /// Being decided: a reference cycle back to it counts as numeric.
    Deciding,
}

/// Where a reference path has arrived after walking its steps.
#[derive(Debug, Clone, Copy)]
pub(super) enum Resolved {
    Comp(CompIdx),
    /// An iteration of a repeat, named by `$r[k]`.
    Iter(CompIdx, ScopeId),
    /// An index with no referent (`$r[32]` with ten iterations).
    Missing,
}

/// One expanded choice.
#[derive(Debug, Clone)]
pub(super) struct ChoiceInst {
    pub(super) def: ChoiceId,
    pub(super) comp: CompIdx,
    /// The scope of each built branch (a select's picks in order; every
    /// case of a reactive choice).
    pub(super) scopes: Vec<ScopeId>,
    /// The branch each of `scopes` instantiates.
    pub(super) branch_of: Vec<usize>,
    /// Reactive choice: one `Choose` component per `ChoiceDef::used`.
    pub(super) iface_comps: Vec<CompIdx>,
}

/// One instantiated template element.
#[derive(Debug, Clone, Copy)]
pub(super) struct Instance {
    pub(super) scope: ScopeId,
    pub(super) template: TemplateId,
    pub(super) elem: ElemId,
    pub(super) comp: CompIdx,
}

pub(super) struct Builder<'c, 'a> {
    pub(super) c: &'c Compiled<'a>,
    pub(super) prior: &'c Prior,
    pub(super) engine: &'c mut dyn SymEngine,
    /// Templates of `Instantiate` instructions, cell leaves holding slots
    /// until emit rebinds them to cells and imports them.
    pub(super) sym_templates: Vec<Tree>,
    /// Slots that hold expression handles without being an instruction's
    /// output (essential and fixed math cells).
    pub(super) math_slots: Vec<SlotId>,
    /// Per component, whether its `expr` is a math cell (`is_symbolic`).
    pub(super) math_mode: Vec<MathMode>,
    pub(super) comps: ComponentTable,
    pub(super) slot_base: Vec<u32>,
    pub(super) sources: Vec<Source>,
    /// Owning component of each slot (prop index = slot - slot_base[comp]).
    pub(super) slot_comp: Vec<CompIdx>,
    pub(super) op_inputs: Vec<SlotId>,
    /// Carried over from the prior build and extended.
    pub(super) scopes: ScopeTable,
    /// Per scope: element -> component, for the scope's template.
    pub(super) scope_comps: Vec<Vec<CompIdx>>,
    /// Per component: index into `instances`, or NONE for synthesized ones.
    pub(super) comp_instance: Vec<u32>,
    pub(super) instances: Vec<Instance>,
    /// Per component: index into `repeats` for a repeat component.
    pub(super) comp_repeat: Vec<u32>,
    pub(super) repeats: Vec<Repeat>,
    pub(super) counts_used: Vec<u32>,
    /// `$ref` children awaiting a kind: (component, plan, scope, has index).
    pub(super) pending: Vec<(CompIdx, RefId, ScopeId, bool)>,
    /// Collect components awaiting expansion.
    pub(super) collects: Vec<CompIdx>,
    /// Point lists awaiting their synthesized children.
    pub(super) pointlists: Vec<CompIdx>,
    pub(super) collected: HashMap<CompIdx, Vec<CompIdx>>,
    /// Expanded choices, and the instance each choice component owns.
    pub(super) choice_insts: Vec<ChoiceInst>,
    pub(super) comp_choice: HashMap<CompIdx, usize>,
    pub(super) missing: Option<SlotId>,
    pub(super) arena: Arena,
    pub(super) root: CompIdx,
}

impl<'c, 'a> Builder<'c, 'a> {
    pub(super) fn new(
        c: &'c Compiled<'a>,
        prior: &'c Prior,
        engine: &'c mut dyn SymEngine,
    ) -> Self {
        // Size the columns from the previous build when there was one.
        let guess = prior
            .structure
            .values
            .iter()
            .map(|v| v.len())
            .sum::<usize>()
            .max(c.templates.iter().map(|t| t.elems.len()).sum::<usize>() * 2);
        Builder {
            c,
            prior,
            engine,
            sym_templates: Vec::new(),
            math_slots: Vec::new(),
            math_mode: Vec::new(),
            comps: ComponentTable {
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
            scopes: prior.structure.scopes.clone(),
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
    pub(super) fn new_component(
        &mut self,
        kind: ComponentKind,
        name: StrId,
        parent: CompIdx,
        node: NodeId,
        scope: ScopeId,
        n_slots: usize,
    ) -> CompIdx {
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
        if name != NONE {
            self.c.dast.strings.get(name).trim().to_string()
        } else {
            format!("<{}>#{}", self.comps.kind[comp as usize].tag(), comp)
        }
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
        self.scopes.get_or_insert(parent, node, k)
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
        self.root = kids
            .iter()
            .copied()
            .find(|&k| k & TEXT_BIT == 0)
            .expect("document root");
        Ok(())
    }

    /// Instantiate template `t` in `scope`; returns the child entries of the
    /// template's own children (the repeat body, or the document).
    pub(super) fn expand(
        &mut self,
        t: TemplateId,
        scope: ScopeId,
        parent: CompIdx,
    ) -> Result<Vec<u32>> {
        let n_elems = self.c.templates[t].elems.len();
        // Create every element's component first so children can refer to them.
        for e in 0..n_elems {
            let el = &self.c.templates[t].elems[e];
            let n_slots = el.props.len().max(el.kind.prop_defs().len());
            let comp = self.new_component(el.kind, el.name, parent, el.node, scope, n_slots);
            self.scope_comps[scope as usize][e] = comp;
            self.comp_instance[comp as usize] = self.instances.len() as u32;
            self.instances.push(Instance {
                scope,
                template: t,
                elem: e,
                comp,
            });
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
                    self.repeats.push(Repeat {
                        comp,
                        node,
                        scope,
                        iter_scopes,
                        n,
                    });
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

    pub(super) fn child_entries(
        &mut self,
        t: TemplateId,
        e: ElemId,
        scope: ScopeId,
        parent: CompIdx,
    ) -> Vec<u32> {
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
        self.scoring_sources();
        Ok(())
    }

    /// Give a `$ref` child its kind: a copy of a component, a number aliasing
    /// one prop, or a number holding the missing-referent cell.
    pub(super) fn place(&mut self, idx: CompIdx, plan: RefId, scope: ScopeId) -> Result<()> {
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
                // A text's value is a string id: it shows as a text.
                let is_text = self
                    .single_component(target, plan)
                    .is_ok_and(|c| self.comps.kind[c as usize] == ComponentKind::Text)
                    && prop == "value";
                self.comps.kind[idx as usize] = if is_text {
                    ComponentKind::Text
                } else {
                    ComponentKind::Number
                };
                self.allocate_slots(idx, 1);
                let s = self.slot(idx, 0);
                self.sources[s as usize] = Source::Alias(targets[0]);
            }
            (target, None) => {
                let referent = self.single_component(target, plan)?;
                let kind = self.comps.kind[referent as usize];
                if matches!(
                    kind,
                    ComponentKind::ConditionalContent | ComponentKind::Select | ComponentKind::Case
                ) {
                    return Err(Error::Banned(format!(
                        "copying a whole <{}>: reference its interface names instead",
                        kind.tag()
                    )));
                }
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
    pub(super) fn resolve_items(
        &mut self,
        plan: RefId,
        scope: ScopeId,
    ) -> Result<Vec<[SlotId; 2]>> {
        let (target, prop) = self.resolve(plan, scope)?;
        let comp = self.single_component(target, plan)?;
        let kind = self.comps.kind[comp as usize];
        let prop = prop.ok_or_else(|| Error::PathTooDeep(self.c.refs[plan].display.clone()))?;
        let items = kind.array_prop(prop).ok_or_else(|| Error::UnknownProp {
            name: self.comp_label(comp),
            prop: prop.into(),
        })?;
        let count_slot = match kind {
            ComponentKind::Polygon => Some(self.slot(comp, prop::polygon::NUM_VERTICES)),
            ComponentKind::Circle => Some(self.slot(comp, prop::circle::NUM_THROUGH_POINTS)),
            _ => None,
        };
        let live = match count_slot.map(|s| self.sources[s as usize].clone()) {
            Some(Source::Fixed(n)) => n as usize,
            _ => items.len(),
        };
        Ok(items
            .iter()
            .take(live)
            .map(|[x, y]| {
                [
                    self.slot(comp, kind.prop_index(x).unwrap()),
                    self.slot(comp, kind.prop_index(y).unwrap()),
                ]
            })
            .collect())
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
                    let pt =
                        self.new_component(ComponentKind::Point, NONE, comp, NONE, inst.scope, 3);
                    let (sx, sy, sh) = (
                        self.slot(pt, prop::point::X),
                        self.slot(pt, prop::point::Y),
                        self.slot(pt, prop::point::HIDE),
                    );
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
                let (s, n) = (
                    self.comps.child_start[referent as usize] as usize,
                    self.comps.child_count[referent as usize] as usize,
                );
                let originals: Vec<CompIdx> = self.comps.child_list[s..s + n].to_vec();
                for orig in originals {
                    let copy = self.new_component(
                        ComponentKind::Document,
                        NONE,
                        comp,
                        NONE,
                        inst.scope,
                        0,
                    );
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
        let Body::Collect { from, kind } = self.c.templates[inst.template].elems[inst.elem].body
        else {
            unreachable!()
        };
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
    pub(super) fn collect_descendants(
        &self,
        c: CompIdx,
        kind: ComponentKind,
        out: &mut Vec<CompIdx>,
    ) {
        let (s, n) = (
            self.comps.child_start[c as usize] as usize,
            self.comps.child_count[c as usize] as usize,
        );
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
        let (s, n) = (
            self.comps.child_start[repeat as usize] as usize,
            self.comps.child_count[repeat as usize] as usize,
        );
        self.comps.child_list[s..s + n]
            .iter()
            .copied()
            .filter(|&e| e & TEXT_BIT == 0 && self.comps.scope[e as usize] == scope)
            .collect()
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
                    return Err(Error::ExtendKindMismatch {
                        referent: self.c.refs[p].display.clone(),
                        referent_kind: rk.tag().into(),
                        kind: kind.tag().into(),
                    });
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
                SourcePlan::Inherit => {
                    Source::Alias(self.slot(extend.expect("Inherit needs an extend referent"), pi))
                }
                SourcePlan::InheritFrom(k) => Source::Alias(self.slot(
                    extend.expect("Inherit needs an extend referent"),
                    *k as usize,
                )),
                SourcePlan::Fixed(v) => Source::Fixed(*v),
                SourcePlan::Alias(arg) => {
                    Source::Alias(self.arg_slot(*arg, comp, inst.scope, kind, pi)?)
                }
                SourcePlan::Op(spec, args) => {
                    let start = self.op_inputs.len() as u32;
                    for &a in args {
                        let slot = self.arg_slot(a, comp, inst.scope, kind, pi)?;
                        self.op_inputs.push(slot);
                    }
                    Source::Op(*spec, start, args.len() as u8)
                }
                SourcePlan::IterIndex => Source::Fixed(self.scopes[inst.scope].2 as f64),
                SourcePlan::Math(expr) => {
                    let id = self.instantiate_expr(*expr, inst.scope)?;
                    if self.arena.is_numeric(id) {
                        Source::Alias(self.lower(id))
                    } else {
                        Source::Fixed(f64::NAN)
                    }
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
                        let expr = self.slot(comp, 0);
                        self.op_source(OpSpec::Sym(SymKind::Evaluate), &[expr])
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
                SourcePlan::VecOut(head, k) => Source::VecOut(self.slot(comp, *head as usize), *k),
            };
            if let (Source::Fixed(h), SourcePlan::MathHandle(..) | SourcePlan::SymExpr(..)) = (
                &source,
                &c.templates[inst.template].elems[inst.elem].props[pi],
            ) && !h.is_nan()
            {
                self.math_slots.push(s);
            }
            self.sources[s as usize] = source;
        }
        Ok(())
    }

    /// An operator source over `inputs` (at most 255).
    pub(super) fn op_source(&mut self, spec: OpSpec, inputs: &[SlotId]) -> Source {
        let start = self.op_inputs.len() as u32;
        self.op_inputs.extend_from_slice(inputs);
        Source::Op(spec, start, inputs.len() as u8)
    }

    pub(super) fn op_slot(&mut self, spec: OpSpec, inputs: &[SlotId]) -> SlotId {
        let source = self.op_source(spec, inputs);
        self.anon_slot(source)
    }

    /// `x`, times `gate` when there is one (the product of the enclosing
    /// cases' `active` cells).
    pub(super) fn gated(&mut self, gate: Option<SlotId>, x: SlotId) -> SlotId {
        match gate {
            None => x,
            Some(g) => self.op_slot(OpSpec::Mul, &[g, x]),
        }
    }
}
