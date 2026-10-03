//! Build a [`Document`] from a flat DAST: create components, expand repeats,
//! resolve references, merge aliased props into shared cells, and emit the
//! instruction list.
//!
//! Phases:
//! A. Walk the DAST, creating a component per element and a placeholder per
//!    `$ref` child. A `<repeatForSequence>` is expanded here: its template is
//!    walked once per iteration, each iteration in its own *scope*, so names
//!    inside the template are unique per iteration. The iteration count comes
//!    from the previous build of the same document (`Prior`), or zero.
//!    Allocate one *slot* per single-cell prop; a component's slots are
//!    contiguous, so `slot = slot_base[comp] + prop_index`.
//! B. Give every slot a source: a literal, a default, a fixed constant, an
//!    alias of another slot, or an operator over other slots. Essential
//!    values saved from the previous build replace literals and defaults.
//! C. Union-find over aliases: each class of aliased slots becomes one cell.
//! D. Number the cells (essential, then fixed, then derived), bind operators,
//!    build the program.
//!
//! Scopes: scope 0 is the document. Each iteration of a repeat is a child
//! scope identified by (parent scope, 1-based position). Names are registered
//! as (scope, name); a bare `$p` is looked up from the referencing component's
//! scope outward, and `$r[3].p` looks `p` up in exactly the iteration's scope.
//! Scope ids are stable across rebuilds of one document (the table only
//! grows), which is how essential keys and iteration counts carry over.

use std::collections::HashMap;

use crate::components::{ComponentKind, PropFrom};
use crate::dast::{Dast, NodeId, NodeKind, StrId, StringTable};
use crate::document::{CellIdx, CompIdx, Components, Document, EssentialKey, Repeat, ScopeId, Structure, NONE, TEXT_BIT};
use crate::error::{Error, Result};
use crate::expr::{Arena, Expr, ExprId, Parser, Token};
use crate::ops::{Instr, OpSpec};
use crate::program::Program;

type SlotId = u32;
type NameId = u32;

#[derive(Debug, Clone)]
enum Source {
    Unset,
    Literal(f64),
    Default(f64),
    /// A constant that is not essential: an iteration index, a collect's
    /// count, the shared missing-referent cell. Requests on it are dropped.
    Fixed(f64),
    Alias(SlotId),
    /// Operator over `op_inputs[start..start + n]`.
    Op(OpSpec, u32, u8),
}

/// What previous builds of the same document contribute to the next:
/// the scope table (ids are stable for the document's lifetime, so a scope
/// that disappears and reappears keeps its id), the iteration count each
/// repeat's `count` cell asks for, and every essential value ever held,
/// so an iteration that comes back comes back as it was left.
#[derive(Debug, Clone, Default)]
pub struct Prior {
    scopes: Vec<(ScopeId, NodeId, u32)>,
    scope_index: HashMap<(ScopeId, NodeId, u32), ScopeId>,
    counts: HashMap<(NodeId, ScopeId), u32>,
    essentials: HashMap<EssentialKey, f64>,
}

impl Prior {
    pub fn from_document(doc: &Document) -> Prior {
        let st = &doc.structure;
        let counts = st.repeats.iter().map(|r| ((r.node, r.scope), doc.repeat_count(r))).collect();
        let mut essentials = st.essential_store.clone();
        for (&key, &v) in st.essential_keys.iter().zip(&doc.cells[..doc.n_essential]) {
            essentials.insert(key, v);
        }
        Prior { scopes: st.scopes.clone(), scope_index: st.scope_index.clone(), counts, essentials }
    }
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
    let mut b = Builder::new(dast, prior);
    b.phase_a()?;
    b.phase_a2_placeholders()?;
    b.phase_b_sources()?;
    b.phase_cd_cells()
}

/// One build pass with nothing carried over: every repeat has zero
/// iterations. `Document::load_timed` iterates this to a fixed point.
pub fn build_once(dast: &Dast) -> Result<Unscheduled> {
    build(dast, &Prior::default())
}

/// Where a reference path has arrived after walking its parts.
#[derive(Debug, Clone, Copy)]
enum Resolved {
    Comp(CompIdx),
    /// An iteration of a repeat, named by `$r[k]`.
    Iter(CompIdx, ScopeId),
    /// An index with no referent (`$r[32]` with ten iterations).
    Missing,
}

struct Builder<'a> {
    dast: &'a Dast,
    prior: &'a Prior,
    comps: Components,
    /// First slot of each component, or NONE until allocated.
    slot_base: Vec<u32>,
    sources: Vec<Source>,
    /// Owning component of each slot (prop index = slot - slot_base[comp]).
    slot_comp: Vec<CompIdx>,
    op_inputs: Vec<SlotId>,
    /// (parent, repeat element, position) per scope; carried over from
    /// `prior` and extended.
    scopes: Vec<(ScopeId, NodeId, u32)>,
    scope_index: HashMap<(ScopeId, NodeId, u32), ScopeId>,
    cur_scope: ScopeId,
    name_ids: HashMap<&'a str, NameId>,
    names: HashMap<(ScopeId, NameId), CompIdx>,
    repeats: Vec<Repeat>,
    counts_used: Vec<u32>,
    /// Explicit elements to process in phase B.
    elements: Vec<(CompIdx, NodeId)>,
    /// `$ref` children awaiting a kind.
    pending: Vec<(CompIdx, NodeId)>,
    /// `<collect>` elements awaiting expansion.
    collects: Vec<(CompIdx, NodeId)>,
    /// Copies made by each collect, for `$c[k]`.
    collected: HashMap<CompIdx, Vec<CompIdx>>,
    missing: Option<SlotId>,
    arena: Arena,
    root: CompIdx,
}

impl<'a> Builder<'a> {
    fn new(dast: &'a Dast, prior: &'a Prior) -> Self {
        let n = dast.len();
        Builder {
            dast,
            prior,
            comps: Components {
                kind: Vec::with_capacity(n),
                name: Vec::with_capacity(n),
                parent: Vec::with_capacity(n),
                prop_base: Vec::new(),
                prop_cells: Vec::new(),
                child_start: Vec::with_capacity(n),
                child_count: Vec::with_capacity(n),
                child_list: Vec::with_capacity(n),
                node: Vec::with_capacity(n),
                scope: Vec::with_capacity(n),
            },
            slot_base: Vec::with_capacity(n),
            sources: Vec::with_capacity(n),
            slot_comp: Vec::with_capacity(n),
            op_inputs: Vec::new(),
            scopes: if prior.scopes.is_empty() { vec![(NONE, NONE, 0)] } else { prior.scopes.clone() },
            scope_index: prior.scope_index.clone(),
            cur_scope: 0,
            name_ids: HashMap::new(),
            names: HashMap::new(),
            repeats: Vec::new(),
            counts_used: Vec::new(),
            elements: Vec::with_capacity(n),
            pending: Vec::new(),
            collects: Vec::new(),
            collected: HashMap::new(),
            missing: None,
            arena: Arena::default(),
            root: 0,
        }
    }

    // ---- Phase A -----------------------------------------------------------

    fn phase_a(&mut self) -> Result<()> {
        let d = self.dast;
        let doc_el = d.children(Dast::ROOT).iter().copied().find(|&n| d.kind(n) == NodeKind::Element && d.str(n) == "document");
        match doc_el {
            Some(el) => self.root = self.add_element(el, NONE)?.unwrap(),
            None => {
                // Synthesize a root when the DAST was not normalized.
                self.root = self.new_component(ComponentKind::Document, NONE, NONE, NONE)?;
                let kids = self.add_children(d.children(Dast::ROOT), self.root)?;
                self.set_children(self.root, &kids);
            }
        }
        Ok(())
    }

    fn name_id(&mut self, s: &'a str) -> NameId {
        let next = self.name_ids.len() as NameId;
        *self.name_ids.entry(s).or_insert(next)
    }

    fn new_component(&mut self, kind: ComponentKind, name: StrId, parent: CompIdx, node: NodeId) -> Result<CompIdx> {
        let idx = self.comps.len() as CompIdx;
        if name != NONE {
            let n = self.dast.strings.get(name).trim();
            let nid = self.name_id(n);
            if self.names.insert((self.cur_scope, nid), idx).is_some() {
                return Err(Error::DuplicateName(self.scoped_label(self.cur_scope, n)));
            }
        }
        self.comps.kind.push(kind);
        self.comps.name.push(name);
        self.comps.parent.push(parent);
        self.comps.child_start.push(0);
        self.comps.child_count.push(0);
        self.comps.node.push(node);
        self.comps.scope.push(self.cur_scope);
        self.slot_base.push(NONE);
        self.allocate_slots(idx);
        Ok(idx)
    }

    fn allocate_slots(&mut self, comp: CompIdx) {
        let n = self.comps.kind[comp as usize].prop_defs().len();
        self.slot_base[comp as usize] = self.sources.len() as u32;
        self.sources.extend(std::iter::repeat_n(Source::Unset, n));
        self.slot_comp.extend(std::iter::repeat_n(comp, n));
    }

    fn slot(&self, comp: CompIdx, prop_index: usize) -> SlotId {
        self.slot_base[comp as usize] + prop_index as u32
    }

    fn set_children(&mut self, comp: CompIdx, kids: &[u32]) {
        self.comps.child_start[comp as usize] = self.comps.child_list.len() as u32;
        self.comps.child_count[comp as usize] = kids.len() as u32;
        self.comps.child_list.extend_from_slice(kids);
    }

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

    /// "r[3].q[2].p" style label for errors.
    fn scoped_label(&self, scope: ScopeId, name: &str) -> String {
        let mut parts = Vec::new();
        let mut s = scope;
        while s != 0 {
            let (parent, _, k) = self.scopes[s as usize];
            // The repeat owning this scope is the one whose iteration scope this is.
            let owner = self.repeats.iter().find(|r| r.scope == parent && r.iter_scopes.contains(&s)).map(|r| self.comp_label(r.comp)).unwrap_or_default();
            parts.push(format!("{owner}[{k}]"));
            s = parent;
        }
        parts.reverse();
        parts.push(name.to_string());
        parts.join(".")
    }

    fn comp_label(&self, comp: CompIdx) -> String {
        let name = self.comps.name[comp as usize];
        if name != NONE {
            self.dast.strings.get(name).trim().to_string()
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

    fn attr_name_str(&self, el: NodeId, attr: &str) -> Option<StrId> {
        let d = self.dast;
        let a = d.attr(el, attr)?;
        match d.attr_children(a) {
            [t] if d.kind(*t) == NodeKind::Text => Some(d.str_id(*t)),
            _ => None,
        }
    }

    fn add_element(&mut self, el: NodeId, parent: CompIdx) -> Result<Option<CompIdx>> {
        let d = self.dast;
        let tag = d.str(el);
        // Normalizer-synthesized elements: `_repeatSetup` holds placeholders
        // for a repeat's valueName/indexName, which the expansion creates itself.
        if tag == "_dynamicChildren" || tag == "_repeatSetup" {
            return Ok(None);
        }
        let kind = ComponentKind::from_tag(tag).ok_or_else(|| Error::UnsupportedTag(tag.to_string()))?;
        let name = self.attr_name_str(el, "name").unwrap_or(NONE);
        let idx = self.new_component(kind, name, parent, el)?;
        self.elements.push((idx, el));
        match kind {
            ComponentKind::RepeatForSequence => self.expand_repeat(idx, el)?,
            ComponentKind::Collect => self.collects.push((idx, el)),
            // A number's children are its value, not rendered children.
            _ if kind.prop_defs().iter().any(|p| p.from == PropFrom::Children) => {}
            _ => {
                let kids = self.add_children(d.children(el), idx)?;
                self.set_children(idx, &kids);
            }
        }
        Ok(Some(idx))
    }

    /// Walk the template once per iteration, each in a fresh scope holding
    /// the iteration's `valueName` and `indexName` components.
    fn expand_repeat(&mut self, idx: CompIdx, el: NodeId) -> Result<()> {
        let d = self.dast;
        let scope = self.cur_scope;
        let n = self.prior.counts.get(&(el, scope)).copied().unwrap_or(0);
        let value_name = self.attr_name_str(el, "valueName");
        let index_name = self.attr_name_str(el, "indexName");
        let template = d.children(el);
        let mut kids = Vec::new();
        let mut iter_scopes = Vec::with_capacity(n as usize);
        for k in 1..=n {
            let s = self.scope_for(scope, el, k);
            iter_scopes.push(s);
            self.cur_scope = s;
            if let Some(name) = value_name {
                let v = self.new_component(ComponentKind::SequenceValue, name, idx, NONE)?;
                let (from, step, kk) = (self.slot(v, 0), self.slot(v, 1), self.slot(v, 2));
                self.sources[from as usize] = Source::Alias(self.slot(idx, 0));
                self.sources[step as usize] = Source::Alias(self.slot(idx, 2));
                self.sources[kk as usize] = Source::Fixed(k as f64);
                self.set_computed_sources(v);
            }
            if let Some(name) = index_name {
                let i = self.new_component(ComponentKind::Number, name, idx, NONE)?;
                let s0 = self.slot(i, 0);
                self.sources[s0 as usize] = Source::Fixed(k as f64);
            }
            let mut these = self.add_children(template, idx)?;
            kids.append(&mut these);
            self.cur_scope = scope;
        }
        self.set_children(idx, &kids);
        self.repeats.push(Repeat { comp: idx, node: el, scope, iter_scopes, n });
        self.counts_used.push(n);
        Ok(())
    }

    /// Sources for the `Computed` props of a synthesized component.
    fn set_computed_sources(&mut self, comp: CompIdx) {
        let kind = self.comps.kind[comp as usize];
        for (pi, def) in kind.prop_defs().iter().enumerate() {
            if let PropFrom::Computed { op, args } = def.from {
                let start = self.op_inputs.len() as u32;
                for &a in args {
                    self.op_inputs.push(self.slot(comp, a as usize));
                }
                let s = self.slot(comp, pi);
                self.sources[s as usize] = Source::Op(op, start, args.len() as u8);
            }
        }
    }

    /// Returns the child list entries for `parent`.
    fn add_children(&mut self, nodes: &[NodeId], parent: CompIdx) -> Result<Vec<u32>> {
        let d = self.dast;
        let mut kids = Vec::with_capacity(nodes.len());
        for &n in nodes {
            match d.kind(n) {
                NodeKind::Element => {
                    if let Some(idx) = self.add_element(n, parent)? {
                        kids.push(idx);
                    }
                }
                // Whitespace-only text between elements carries no content and
                // would otherwise become one DOM node per element.
                NodeKind::Text if d.str(n).trim().is_empty() => {}
                NodeKind::Text => kids.push(TEXT_BIT | d.str_id(n)),
                NodeKind::Macro => {
                    // Kind is unknown until names resolve; Document stands in.
                    let idx = self.new_component(ComponentKind::Document, NONE, parent, NONE)?;
                    kids.push(idx);
                    self.pending.push((idx, n));
                }
                NodeKind::Other => {}
            }
        }
        Ok(kids)
    }

    // ---- Phase A2: collects, then `$ref` children -------------------------

    fn phase_a2_placeholders(&mut self) -> Result<()> {
        let d = self.dast;
        // Placeholders without an index first (a collect may gather copies),
        // then collects in document order, then indexed placeholders (which
        // may name a collect's items).
        let pending = std::mem::take(&mut self.pending);
        let (plain, indexed): (Vec<_>, Vec<_>) = pending.into_iter().partition(|&(_, m)| !d.macro_has_index(m));
        for (idx, m) in plain {
            self.place(idx, m)?;
        }
        let collects = std::mem::take(&mut self.collects);
        for (idx, el) in collects {
            self.expand_collect(idx, el)?;
        }
        for (idx, m) in indexed {
            self.place(idx, m)?;
        }
        Ok(())
    }

    /// Give a `$ref` child its kind: a copy of a component, a number aliasing
    /// one prop, or a number holding the missing-referent cell.
    fn place(&mut self, idx: CompIdx, m: NodeId) -> Result<()> {
        let scope = self.comps.scope[idx as usize];
        let (target, prop) = self.walk(m, scope)?;
        match (target, prop) {
            (Resolved::Missing, _) => {
                self.comps.kind[idx as usize] = ComponentKind::Number;
                self.allocate_slots(idx);
                let s = self.slot(idx, 0);
                self.sources[s as usize] = Source::Alias(self.missing_slot());
            }
            (_, Some(prop)) => {
                let targets = self.targets_of(target, Some(prop), m, Some(1))?;
                self.comps.kind[idx as usize] = ComponentKind::Number;
                self.allocate_slots(idx);
                let s = self.slot(idx, 0);
                self.sources[s as usize] = Source::Alias(targets[0]);
            }
            (target, None) => {
                let referent = self.single_component(target, m)?;
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

    fn expand_collect(&mut self, idx: CompIdx, el: NodeId) -> Result<()> {
        let d = self.dast;
        let from = d.attr(el, "from").and_then(|a| self.single_macro(a)).ok_or(Error::BadCollect)?;
        let type_text = d.attr(el, "componentType").and_then(|a| self.attr_text(a)).ok_or(Error::BadCollect)?;
        let kind = ComponentKind::from_tag(type_text.trim()).filter(|k| k.collectable()).ok_or_else(|| Error::BadCollectType(type_text.trim().into()))?;
        let scope = self.comps.scope[idx as usize];
        let (target, prop) = self.walk(from, scope)?;
        if prop.is_some() {
            return Err(Error::BadCollect);
        }
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
        let saved = self.cur_scope;
        self.cur_scope = scope;
        let mut copies = Vec::with_capacity(found.len());
        for &c in &found {
            // Created kind-less so `copy_into` allocates the slots once.
            let copy = self.new_component(ComponentKind::Document, NONE, idx, NONE)?;
            self.copy_into(copy, c);
            copies.push(copy);
        }
        self.cur_scope = saved;
        self.set_children(idx, &copies);
        let s0 = self.slot(idx, 0);
        self.sources[s0 as usize] = Source::Fixed(copies.len() as f64);
        self.collected.insert(idx, copies);
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

    fn missing_slot(&mut self) -> SlotId {
        if let Some(s) = self.missing {
            return s;
        }
        // A fixed NaN cell shared by every reference without a referent.
        let s = self.anon_slot(Source::Fixed(f64::NAN));
        self.missing = Some(s);
        s
    }

    /// A slot that belongs to no component: a lowered math subexpression,
    /// a literal inside one, the missing-referent cell.
    fn anon_slot(&mut self, source: Source) -> SlotId {
        let s = self.sources.len() as SlotId;
        self.sources.push(source);
        self.slot_comp.push(NONE);
        s
    }

    // ---- Phase B: sources for explicit elements -----------------------------

    fn phase_b_sources(&mut self) -> Result<()> {
        let d = self.dast;
        let elements = std::mem::take(&mut self.elements);
        for (idx, el) in elements {
            let kind = self.comps.kind[idx as usize];
            let scope = self.comps.scope[idx as usize];
            let extend = match d.attr(el, "extend") {
                Some(a) => {
                    let m = self.single_macro(a).ok_or_else(|| Error::BadValue { attr: "extend".into(), text: self.attr_text(a).unwrap_or_default() })?;
                    let (target, prop) = self.walk(m, scope)?;
                    if prop.is_some() {
                        return Err(Error::PathTooDeep(d.macro_display(m)));
                    }
                    let referent = self.single_component(target, m)?;
                    let rk = self.comps.kind[referent as usize];
                    if rk != kind {
                        return Err(Error::ExtendKindMismatch { referent: d.macro_display(m), referent_kind: rk.tag().into(), kind: kind.tag().into() });
                    }
                    Some(referent)
                }
                None => None,
            };

            if kind == ComponentKind::Math {
                self.math_sources(idx, el, scope)?;
                continue;
            }

            // Virtual multi-cell attributes (a point's coords) bind several slots at once.
            let mut bound_by_virtual = vec![false; kind.prop_defs().len()];
            for (vname, parts) in virtual_attrs(kind) {
                if let Some(a) = d.attr(el, vname) {
                    let m = self.single_macro(a).ok_or_else(|| Error::BadValue { attr: vname.into(), text: self.attr_text(a).unwrap_or_default() })?;
                    let targets = self.resolve_ref(m, scope, Some(parts.len()))?;
                    if targets.len() != parts.len() {
                        return Err(Error::ArityMismatch { kind: kind.tag().into(), prop: vname.into(), expected: parts.len(), got: targets.len() });
                    }
                    for (part, &t) in parts.iter().zip(&targets) {
                        let pi = kind.prop_index(part).unwrap();
                        if d.attr(el, part).is_some() {
                            return Err(Error::BadValue { attr: part.to_string(), text: format!("conflicts with {vname}") });
                        }
                        let s = self.slot(idx, pi);
                        self.sources[s as usize] = Source::Alias(t);
                        bound_by_virtual[pi] = true;
                    }
                }
            }

            for (pi, def) in kind.prop_defs().iter().enumerate() {
                if bound_by_virtual[pi] {
                    continue;
                }
                let s = self.slot(idx, pi);
                if !matches!(self.sources[s as usize], Source::Unset) {
                    continue; // set during expansion (a collect's count)
                }
                let bound = match def.bind.and_then(|b| d.attr(el, b)) {
                    Some(a) => {
                        let bind = def.bind.unwrap();
                        let m = self.single_macro(a).ok_or_else(|| Error::BadValue { attr: bind.into(), text: self.attr_text(a).unwrap_or_default() })?;
                        let targets = self.resolve_ref(m, scope, Some(1))?;
                        if targets.len() != 1 {
                            return Err(Error::ArityMismatch { kind: kind.tag().into(), prop: bind.into(), expected: 1, got: targets.len() });
                        }
                        Some(Source::Alias(targets[0]))
                    }
                    None => None,
                };
                let source = match (bound, def.from) {
                    (Some(src), _) => src,
                    (None, PropFrom::Attribute) => match d.attr(el, def.attr_name()) {
                        Some(a) => self.value_source_with(kind, def.attr_name(), d.attr_children(a), scope, def.ref_prop)?,
                        None => self.inherit_or_default(extend, pi, def.default),
                    },
                    (None, PropFrom::AttributeOr { alias }) => match d.attr(el, def.attr_name()) {
                        Some(a) => self.value_source(kind, def.attr_name(), d.attr_children(a), scope)?,
                        None => Source::Alias(self.slot(idx, alias as usize)),
                    },
                    (None, PropFrom::Computed { op, args }) => {
                        let start = self.op_inputs.len() as u32;
                        for &a in args {
                            self.op_inputs.push(self.slot(idx, a as usize));
                        }
                        Source::Op(op, start, args.len() as u8)
                    }
                    (None, PropFrom::Children) => {
                        if d.children(el).iter().all(|&n| self.is_blank(n)) {
                            self.inherit_or_default(extend, pi, def.default)
                        } else {
                            match self.value_source(kind, def.name, d.children(el), scope) {
                                Ok(src) => src,
                                // Not a single literal or reference: a math
                                // expression, lowered to operators (NaN if symbolic).
                                Err(Error::BadValue { .. }) => {
                                    let id = self.parse_math(d.children(el), scope)?;
                                    if self.arena.is_numeric(id) {
                                        Source::Alias(self.lower(id))
                                    } else {
                                        Source::Fixed(f64::NAN)
                                    }
                                }
                                Err(e) => return Err(e),
                            }
                        }
                    }
                    (None, PropFrom::Derived) => self.op_source(el, scope)?,
                };
                self.sources[s as usize] = source;
            }
            if kind == ComponentKind::Evaluate {
                self.evaluate_sources(idx);
            }
        }
        Ok(())
    }

    fn inherit_or_default(&self, extend: Option<CompIdx>, pi: usize, default: f64) -> Source {
        match extend {
            Some(r) => Source::Alias(self.slot(r, pi)),
            None => Source::Default(default),
        }
    }

    /// A literal number or a single reference, as found in an attribute or in
    /// a number's children.
    fn value_source(&mut self, kind: ComponentKind, attr: &str, nodes: &[NodeId], scope: ScopeId) -> Result<Source> {
        self.value_source_with(kind, attr, nodes, scope, None)
    }

    fn value_source_with(&mut self, kind: ComponentKind, attr: &str, nodes: &[NodeId], scope: ScopeId, ref_prop: Option<&str>) -> Result<Source> {
        let d = self.dast;
        let macros: Vec<NodeId> = nodes.iter().copied().filter(|&n| d.kind(n) == NodeKind::Macro).collect();
        let text: String = nodes.iter().filter(|&&n| d.kind(n) == NodeKind::Text).map(|&n| d.str(n)).collect();
        let text = text.trim();
        match (macros.len(), text.is_empty()) {
            (1, true) => {
                let targets = self.resolve_ref_with(macros[0], scope, Some(1), ref_prop)?;
                if targets.len() != 1 {
                    return Err(Error::ArityMismatch { kind: kind.tag().into(), prop: attr.into(), expected: 1, got: targets.len() });
                }
                Ok(Source::Alias(targets[0]))
            }
            (0, false) => match text {
                "true" => Ok(Source::Literal(1.0)),
                "false" => Ok(Source::Literal(0.0)),
                _ => text.parse::<f64>().map(Source::Literal).map_err(|_| Error::BadValue { attr: attr.into(), text: text.into() }),
            },
            _ => Err(Error::BadValue { attr: attr.into(), text: text.into() }),
        }
    }

    fn op_source(&mut self, el: NodeId, scope: ScopeId) -> Result<Source> {
        let d = self.dast;
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
            "lerp" => OpSpec::Lerp { t: param("t")? },
            other => return Err(Error::UnknownOp(other.into())),
        };
        let start = self.op_inputs.len() as u32;
        let mut count = 0usize;
        if let Some(args) = d.attr(el, "args") {
            for &node in d.attr_children(args) {
                match d.kind(node) {
                    NodeKind::Macro => {
                        let targets = self.resolve_ref(node, scope, Some(1))?;
                        if targets.len() != 1 {
                            return Err(Error::ArityMismatch { kind: "op".into(), prop: "args".into(), expected: 1, got: targets.len() });
                        }
                        self.op_inputs.push(targets[0]);
                        count += 1;
                    }
                    NodeKind::Text if d.str(node).trim().is_empty() => {}
                    NodeKind::Text => return Err(Error::LiteralArg),
                    _ => {}
                }
            }
        }
        if count != spec.arity() {
            return Err(Error::OpArity { kind: kind_text.into(), expected: spec.arity(), got: count });
        }
        Ok(Source::Op(spec, start, count as u8))
    }

    // ---- reference resolution ---------------------------------------------

    /// Look a bare name up from `scope` outward to the document scope.
    fn lookup(&self, name: &str, scope: ScopeId) -> Result<CompIdx> {
        let Some(&nid) = self.name_ids.get(name.trim()) else { return Err(Error::UnknownName(name.into())) };
        let mut s = scope;
        loop {
            if let Some(&c) = self.names.get(&(s, nid)) {
                return Ok(c);
            }
            if s == 0 {
                return Err(Error::UnknownName(name.into()));
            }
            s = self.scopes[s as usize].0;
        }
    }

    /// Look a name up in exactly one scope (`$r[3].p`).
    fn lookup_in(&self, name: &str, scope: ScopeId) -> Option<CompIdx> {
        let nid = *self.name_ids.get(name.trim())?;
        self.names.get(&(scope, nid)).copied()
    }

    /// Walk a macro path. Returns where it arrived and, if the final part
    /// named a prop of a component, that prop.
    fn walk(&self, m: NodeId, scope: ScopeId) -> Result<(Resolved, Option<&'a str>)> {
        let d = self.dast;
        let names = d.macro_path(m);
        let parts: Vec<_> = d.macro_parts(m).collect();
        let mut cur = Resolved::Comp(self.lookup(d.strings.get(names[0]), scope)?);
        for (i, &part) in parts.iter().enumerate() {
            if i > 0 {
                let name = d.strings.get(names[i]);
                cur = match cur {
                    Resolved::Missing => Resolved::Missing,
                    Resolved::Iter(_, s) => Resolved::Comp(self.lookup_in(name, s).ok_or_else(|| Error::UnknownName(d.macro_display(m)))?),
                    Resolved::Comp(c) => {
                        // A prop name: must be the last part and carry no index.
                        if i + 1 != parts.len() || d.part_indices(part).next().is_some() {
                            return Err(Error::PathTooDeep(d.macro_display(m)));
                        }
                        let kind = self.comps.kind[c as usize];
                        if kind.prop_index(name).is_none() && kind.virtual_prop(name).is_none() {
                            return Err(Error::UnknownProp { name: self.comp_label(c), prop: name.into() });
                        }
                        return Ok((Resolved::Comp(c), Some(name)));
                    }
                };
            }
            for expr in d.part_indices(part) {
                cur = self.index_into(cur, expr, scope, m)?;
            }
        }
        Ok((cur, None))
    }

    fn index_into(&self, cur: Resolved, expr: &[NodeId], scope: ScopeId, m: NodeId) -> Result<Resolved> {
        let k = self.index_value(expr, scope, m)?;
        Ok(match cur {
            Resolved::Missing => Resolved::Missing,
            Resolved::Iter(..) => return Err(Error::NotIndexable(self.dast.macro_display(m))),
            Resolved::Comp(c) => match self.comps.kind[c as usize] {
                ComponentKind::RepeatForSequence => {
                    let r = self.repeats.iter().find(|r| r.comp == c).unwrap();
                    if k < 1 || k > r.n as i64 {
                        Resolved::Missing
                    } else {
                        Resolved::Iter(c, r.iter_scopes[(k - 1) as usize])
                    }
                }
                ComponentKind::Collect => match self.collected.get(&c) {
                    Some(items) if k >= 1 && (k as usize) <= items.len() => Resolved::Comp(items[(k - 1) as usize]),
                    Some(_) => Resolved::Missing,
                    None => return Err(Error::NotIndexable(self.dast.macro_display(m))),
                },
                _ => return Err(Error::NotIndexable(self.dast.macro_display(m))),
            },
        })
    }

    /// Evaluate an index expression at build time: a sum of literal integers
    /// and fixed iteration indices (`3`, `$i`, `$i-2`).
    fn index_value(&self, expr: &[NodeId], scope: ScopeId, m: NodeId) -> Result<i64> {
        let d = self.dast;
        let mut total = 0i64;
        for &n in expr {
            match d.kind(n) {
                NodeKind::Text => {
                    let t: String = d.str(n).chars().filter(|c| !c.is_whitespace()).collect();
                    if t.is_empty() {
                        continue;
                    }
                    total += t.trim_start_matches('+').parse::<i64>().map_err(|_| Error::BadIndex(d.macro_display(m)))?;
                }
                NodeKind::Macro => {
                    let path = d.macro_path(n);
                    if path.len() != 1 || d.macro_has_index(n) {
                        return Err(Error::DynamicIndex(d.macro_display(m)));
                    }
                    let c = self.lookup(d.strings.get(path[0]), scope)?;
                    let kind = self.comps.kind[c as usize];
                    if kind.prop_defs().len() != 1 {
                        return Err(Error::DynamicIndex(d.macro_display(m)));
                    }
                    match self.sources[self.slot(c, 0) as usize] {
                        Source::Fixed(v) if v.fract() == 0.0 => total += v as i64,
                        _ => return Err(Error::DynamicIndex(d.macro_display(m))),
                    }
                }
                _ => {}
            }
        }
        Ok(total)
    }

    /// The one component an unindexed path or `$r[k]` denotes.
    fn single_component(&self, target: Resolved, m: NodeId) -> Result<CompIdx> {
        match target {
            Resolved::Comp(c) => Ok(c),
            Resolved::Iter(repeat, s) => {
                let comps = self.iteration_components(repeat, s);
                if comps.len() == 1 { Ok(comps[0]) } else { Err(Error::AmbiguousIteration(self.dast.macro_display(m), comps.len())) }
            }
            Resolved::Missing => Err(Error::UnknownName(self.dast.macro_display(m))),
        }
    }

    /// Slots named by a resolved path, using the default prop when none was given.
    fn targets_of(&mut self, target: Resolved, prop: Option<&str>, m: NodeId, expected: Option<usize>) -> Result<Vec<SlotId>> {
        let comp = match target {
            Resolved::Missing => {
                let s = self.missing_slot();
                return Ok(vec![s; expected.unwrap_or(1)]);
            }
            other => self.single_component(other, m)?,
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

    /// Slots named by `$name`, `$name.prop`, `$r[k].name.prop` and so on.
    /// A reference with no referent yields `expected` copies of the shared
    /// missing cell.
    fn resolve_ref(&mut self, m: NodeId, scope: ScopeId, expected: Option<usize>) -> Result<Vec<SlotId>> {
        self.resolve_ref_with(m, scope, expected, None)
    }

    /// `resolve_ref`, but a bare component reference takes `ref_prop`
    /// instead of the component's default prop when given.
    fn resolve_ref_with(&mut self, m: NodeId, scope: ScopeId, expected: Option<usize>, ref_prop: Option<&str>) -> Result<Vec<SlotId>> {
        let (target, prop) = self.walk(m, scope)?;
        self.targets_of(target, prop.or(ref_prop), m, expected)
    }

    // ---- math: parse, lower, or keep symbolic -----------------------------

    /// `<math>`: `expr` holds the arena handle; `value` is the lowered chain
    /// when the expression is numeric, else an `Evaluate` of the handle.
    fn math_sources(&mut self, idx: CompIdx, el: NodeId, scope: ScopeId) -> Result<()> {
        let d = self.dast;
        let id = self.parse_math(d.children(el), scope)?;
        let expr_slot = self.slot(idx, 0);
        let value_slot = self.slot(idx, 1);
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
        Ok(())
    }

    /// `<evaluate>`: add the function expression's cell leaves to the
    /// `EvalAt` instruction's inputs, so changes to them propagate.
    fn evaluate_sources(&mut self, idx: CompIdx) {
        let (func, input, value) = (self.slot(idx, 0), self.slot(idx, 1), self.slot(idx, 2));
        // Follow aliases from `function` to the math's fixed handle.
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

    /// Text and `$ref` children to an arena expression; references become
    /// cell leaves.
    fn parse_math(&mut self, nodes: &[NodeId], scope: ScopeId) -> Result<ExprId> {
        let d = self.dast;
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
                    let targets = self.resolve_ref(n, scope, Some(1))?;
                    if targets.len() != 1 {
                        return Err(Error::BadMath { text, reason: "a reference inside math must name one cell".into() });
                    }
                    toks.push(Token::Cell(targets[0]));
                }
                _ => {}
            }
        }
        Parser::parse(&toks, &mut self.arena).map_err(|reason| Error::BadMath { text: text.trim().to_string(), reason })
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

    fn attr_text(&self, a: u32) -> Option<String> {
        let d = self.dast;
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
        let d = self.dast;
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
        match self.dast.kind(n) {
            NodeKind::Text => self.dast.str(n).trim().is_empty(),
            NodeKind::Other => true,
            _ => false,
        }
    }

    // ---- Phases C and D: alias classes become cells ------------------------

    fn phase_cd_cells(mut self) -> Result<Unscheduled> {
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
        for s in 0..n {
            if class_def[uf.find(s as SlotId) as usize] == NONE {
                return Err(Error::Cycle(self.slot_label(s as SlotId)));
            }
        }

        // Number cells: essential classes first, then fixed, then derived.
        let mut slot_cell: Vec<CellIdx> = vec![NONE; n];
        let mut cells = Vec::new();
        let mut cell_def_slot: Vec<SlotId> = Vec::new();
        let mut essential_keys: Vec<EssentialKey> = Vec::new();
        let mut fixed_defs = Vec::new();
        let mut derived_defs = Vec::new();
        for s in 0..n {
            let root = uf.find(s as SlotId) as usize;
            if class_def[root] as usize != s {
                continue; // not the defining slot of its class
            }
            match &self.sources[s] {
                Source::Literal(v) | Source::Default(v) => {
                    let key = self.essential_key(s as SlotId);
                    let value = self.prior.essentials.get(&key).copied().unwrap_or(*v);
                    slot_cell[root] = cells.len() as CellIdx;
                    cells.push(value);
                    cell_def_slot.push(s as SlotId);
                    essential_keys.push(key);
                }
                Source::Fixed(_) => fixed_defs.push(s),
                Source::Op(..) => derived_defs.push(s),
                _ => unreachable!(),
            }
        }
        let n_essential = cells.len();
        for &s in &fixed_defs {
            let Source::Fixed(v) = self.sources[s] else { unreachable!() };
            let root = uf.find(s as SlotId) as usize;
            slot_cell[root] = cells.len() as CellIdx;
            cells.push(v);
            cell_def_slot.push(s as SlotId);
        }
        let n_fixed = fixed_defs.len();
        for &s in &derived_defs {
            let root = uf.find(s as SlotId) as usize;
            slot_cell[root] = cells.len() as CellIdx;
            cells.push(f64::NAN);
            cell_def_slot.push(s as SlotId);
        }
        let cell_of = |uf: &mut UnionFind, slot: SlotId| slot_cell[uf.find(slot) as usize];

        let mut instrs = Vec::with_capacity(derived_defs.len());
        let mut extra = Vec::new();
        for &s in &derived_defs {
            let Source::Op(spec, start, count) = &self.sources[s] else { unreachable!() };
            let inputs = &self.op_inputs[*start as usize..*start as usize + *count as usize];
            let bound: Vec<CellIdx> = inputs.iter().map(|&i| cell_of(&mut uf, i)).collect();
            instrs.push(Instr { out: cell_of(&mut uf, s as SlotId), op: spec.bind(&bound, &mut extra) });
        }

        // Prop cells: contiguous per component, in slot order.
        let n_comps = self.comps.len();
        self.comps.prop_base = Vec::with_capacity(n_comps);
        self.comps.prop_cells = Vec::with_capacity(n);
        for c in 0..n_comps {
            let np = self.comps.kind[c].prop_defs().len();
            self.comps.prop_base.push(self.comps.prop_cells.len() as u32);
            for pi in 0..np {
                let s = self.slot_base[c] + pi as u32;
                self.comps.prop_cells.push(cell_of(&mut uf, s));
            }
        }

        let (depths, cross_reads) = self.structural_depths();

        // Cell leaves in the arena were slots; they are cells now.
        let mut arena = self.arena;
        let slot_to_cell: Vec<CellIdx> = (0..n as SlotId).map(|s| cell_of(&mut uf, s)).collect();
        arena.map_cells(|slot| slot_to_cell[slot as usize]);

        let structure = Structure {
            scopes: self.scopes,
            scope_index: self.scope_index,
            essential_keys,
            essential_store: self.prior.essentials.clone(),
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
        let names: Vec<Option<String>> = self.comps.name.iter().map(|&s| (s != NONE).then(|| self.dast.strings.get(s).trim().to_string())).collect();
        let cell_label = Box::new(move |cell: CellIdx| {
            let slot = cell_def_slot[cell as usize];
            let comp = slot_comp[slot as usize];
            if comp == NONE {
                return "(missing referent)".to_string();
            }
            let pi = (slot - slot_base[comp as usize]) as usize;
            let owner = names[comp as usize].clone().unwrap_or_else(|| format!("<{}>#{}", kinds[comp as usize].tag(), comp));
            format!("{owner}.{}", kinds[comp as usize].prop_defs()[pi].name)
        });

        // Columns were sized by DAST node count, an upper bound; release the slack.
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
        Ok(Unscheduled { cells, n_essential, n_fixed, instrs, comps, strings: self.dast.strings.clone(), root: self.root, structure, arena, extra, cell_label })
    }

    /// Structural depth per repeat: how many repeats must be expanded, in
    /// sequence, before its count can be computed, plus one. A top-level
    /// repeat whose count reads only cells outside iterations has depth 1; a
    /// nested repeat is one deeper than its enclosing repeat (its count
    /// cannot exist until the enclosing iteration does); a count that reads
    /// a cell inside another repeat's iterations is one deeper than that
    /// repeat. Load takes depth + 1 passes, and a structural tick re-expands
    /// along the chain. The second kind of link is also flagged (`cross`),
    /// since it is the one authors can avoid. The missing-referent cell
    /// belongs to no scope, so a link only counts once the referenced
    /// iteration exists, which is what makes the settled build's depth the
    /// true one.
    fn structural_depths(&self) -> (Vec<u32>, Vec<bool>) {
        let count_pi = ComponentKind::RepeatForSequence.prop_index("count").unwrap();
        // Repeat owning each iteration scope.
        let mut owner: HashMap<ScopeId, usize> = HashMap::new();
        for (ri, r) in self.repeats.iter().enumerate() {
            for &s in &r.iter_scopes {
                owner.insert(s, ri);
            }
        }
        // Whether scope `s` is `of` or one of its ancestors.
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
        // Edges: repeat -> repeats that must be expanded before its count
        // can be computed. Nesting is one such edge (the enclosing
        // iteration has to exist); a count reading a cell inside another
        // repeat's iterations is the other, and the one worth a warning.
        let mut reads: Vec<Vec<usize>> = vec![Vec::new(); self.repeats.len()];
        let mut cross = vec![false; self.repeats.len()];
        let mut seen = vec![false; self.sources.len()];
        for (ri, r) in self.repeats.iter().enumerate() {
            if let Some(&o) = owner.get(&r.scope) {
                reads[ri].push(o);
            }
            let mut stack = vec![self.slot(r.comp, count_pi)];
            for v in seen.iter_mut() {
                *v = false;
            }
            while let Some(sl) = stack.pop() {
                if seen[sl as usize] {
                    continue;
                }
                seen[sl as usize] = true;
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
        }
        // Longest chain, memoized; a cycle among counts is capped rather than followed.
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

    /// The key an essential slot's value is saved under across rebuilds: the
    /// DAST element, the prop, and the (stable) scope.
    fn essential_key(&self, slot: SlotId) -> EssentialKey {
        let comp = self.slot_comp[slot as usize];
        let pi = (slot - self.slot_base[comp as usize]) as u8;
        EssentialKey { node: self.comps.node[comp as usize], prop: pi, scope: self.comps.scope[comp as usize] }
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
