//! Build a [`Document`] from a flat DAST: create components, resolve
//! references, merge aliased props into shared cells, and emit the
//! instruction list.
//!
//! Phases:
//! A. Walk the DAST, creating a component per element and a placeholder per
//!    `$ref` child. Allocate one *slot* per single-cell prop; a component's
//!    slots are contiguous, so `slot = slot_base[comp] + prop_index`.
//! B. Give every slot a source: a literal, a default, an alias of another
//!    slot, or an operator over other slots.
//! C. Union-find over aliases: each class of aliased slots becomes one cell.
//! D. Number the cells (essential first), bind operators, build the program.

use std::collections::HashMap;

use crate::components::{ComponentKind, PropFrom};
use crate::dast::{Dast, NodeId, NodeKind, StrId, StringTable};
use crate::document::{CellIdx, CompIdx, Components, Document, NONE, TEXT_BIT};
use crate::error::{Error, Result};
use crate::ops::{Instr, OpSpec};
use crate::program::Program;

type SlotId = u32;

#[derive(Debug, Clone)]
enum Source {
    Unset,
    Literal(f64),
    Default(f64),
    Alias(SlotId),
    /// Operator over `op_inputs[start..start + n]`.
    Op(OpSpec, u32, u8),
}

/// A built document whose program has not yet been scheduled.
pub struct Unscheduled {
    cells: Vec<f64>,
    n_essential: usize,
    instrs: Vec<Instr>,
    comps: Components,
    strings: StringTable,
    root: CompIdx,
    /// Human-readable owner of a cell, e.g. "p1.x". Computed lazily because
    /// a cycle error is the only consumer.
    cell_label: Box<dyn Fn(CellIdx) -> String>,
}

impl Unscheduled {
    pub fn schedule(self) -> Result<Document> {
        let n = self.cells.len();
        let program = Program::schedule(self.instrs, n).map_err(|cell| Error::Cycle((self.cell_label)(cell)))?;
        Ok(Document::new(self.cells, self.n_essential, program, self.comps, self.strings, self.root))
    }
}

pub fn build(dast: &Dast) -> Result<Unscheduled> {
    let mut b = Builder::new(dast);
    b.phase_a()?;
    b.phase_a2_placeholders()?;
    b.phase_b_sources()?;
    b.phase_cd_cells()
}

struct Builder<'a> {
    dast: &'a Dast,
    comps: Components,
    /// First slot of each component, or NONE until allocated.
    slot_base: Vec<u32>,
    sources: Vec<Source>,
    /// Owning component of each slot (prop index = slot - slot_base[comp]).
    slot_comp: Vec<CompIdx>,
    op_inputs: Vec<SlotId>,
    names: HashMap<&'a str, CompIdx>,
    /// Explicit elements to process in phase B.
    elements: Vec<(CompIdx, NodeId)>,
    /// `$ref` children awaiting a kind.
    pending: Vec<(CompIdx, NodeId)>,
    root: CompIdx,
}

impl<'a> Builder<'a> {
    fn new(dast: &'a Dast) -> Self {
        let n = dast.len();
        Builder {
            dast,
            comps: Components {
                kind: Vec::with_capacity(n),
                name: Vec::with_capacity(n),
                parent: Vec::with_capacity(n),
                prop_base: Vec::new(),
                prop_cells: Vec::new(),
                child_start: Vec::with_capacity(n),
                child_count: Vec::with_capacity(n),
                child_list: Vec::with_capacity(n),
            },
            slot_base: Vec::with_capacity(n),
            sources: Vec::with_capacity(n),
            slot_comp: Vec::with_capacity(n),
            op_inputs: Vec::new(),
            names: HashMap::new(),
            elements: Vec::with_capacity(n),
            pending: Vec::new(),
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
                self.root = self.new_component(ComponentKind::Document, NONE, NONE)?;
                let kids = self.add_children(d.children(Dast::ROOT), self.root)?;
                self.set_children(self.root, &kids);
            }
        }
        Ok(())
    }

    fn new_component(&mut self, kind: ComponentKind, name: StrId, parent: CompIdx) -> Result<CompIdx> {
        let idx = self.comps.len() as CompIdx;
        if name != NONE {
            let n = self.dast.strings.get(name).trim();
            if self.names.insert(n, idx).is_some() {
                return Err(Error::DuplicateName(n.to_string()));
            }
        }
        self.comps.kind.push(kind);
        self.comps.name.push(name);
        self.comps.parent.push(parent);
        self.comps.child_start.push(0);
        self.comps.child_count.push(0);
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
        let pi = (slot - self.slot_base[comp as usize]) as usize;
        format!("{}.{}", self.comp_label(comp), self.comps.kind[comp as usize].prop_defs()[pi].name)
    }

    fn add_element(&mut self, el: NodeId, parent: CompIdx) -> Result<Option<CompIdx>> {
        let d = self.dast;
        let tag = d.str(el);
        if tag == "_dynamicChildren" {
            return Ok(None);
        }
        let kind = ComponentKind::from_tag(tag).ok_or_else(|| Error::UnsupportedTag(tag.to_string()))?;
        let name = match d.attr(el, "name") {
            Some(a) => match d.attr_children(a) {
                [t] if d.kind(*t) == NodeKind::Text => d.str_id(*t),
                _ => NONE,
            },
            None => NONE,
        };
        let idx = self.new_component(kind, name, parent)?;
        self.elements.push((idx, el));
        // A number's children are its value, not rendered children.
        if !kind.prop_defs().iter().any(|p| p.from == PropFrom::Children) {
            let kids = self.add_children(d.children(el), idx)?;
            self.set_children(idx, &kids);
        }
        Ok(Some(idx))
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
                    let idx = self.new_component(ComponentKind::Document, NONE, parent)?;
                    kids.push(idx);
                    self.pending.push((idx, n));
                }
                NodeKind::Other => {}
            }
        }
        Ok(kids)
    }

    // ---- Phase A2: give `$ref` children a kind and alias their props -------

    fn phase_a2_placeholders(&mut self) -> Result<()> {
        let d = self.dast;
        let pending = std::mem::take(&mut self.pending);
        for (idx, m) in pending {
            let path = d.macro_path(m);
            match path.len() {
                1 => {
                    let referent = self.lookup(d.strings.get(path[0]))?;
                    let kind = self.comps.kind[referent as usize];
                    if !kind.copyable() {
                        return Err(Error::UncopyableKind(kind.tag().into()));
                    }
                    self.comps.kind[idx as usize] = kind;
                    self.allocate_slots(idx);
                    for pi in 0..kind.prop_defs().len() {
                        let s = self.slot(idx, pi);
                        self.sources[s as usize] = Source::Alias(self.slot(referent, pi));
                    }
                }
                2 => {
                    self.comps.kind[idx as usize] = ComponentKind::Number;
                    self.allocate_slots(idx);
                    let targets = self.resolve_ref(m)?;
                    if targets.len() != 1 {
                        return Err(Error::ArityMismatch { kind: "number".into(), prop: "value".into(), expected: 1, got: targets.len() });
                    }
                    let s = self.slot(idx, 0);
                    self.sources[s as usize] = Source::Alias(targets[0]);
                }
                _ => return Err(Error::PathTooDeep(d.macro_display(m))),
            }
        }
        Ok(())
    }

    // ---- Phase B: sources for explicit elements -----------------------------

    fn phase_b_sources(&mut self) -> Result<()> {
        let d = self.dast;
        let elements = std::mem::take(&mut self.elements);
        for (idx, el) in elements {
            let kind = self.comps.kind[idx as usize];
            let extend = match d.attr(el, "extend") {
                Some(a) => {
                    let m = self.single_macro(a).ok_or_else(|| Error::BadValue { attr: "extend".into(), text: self.attr_text(a).unwrap_or_default() })?;
                    let path = d.macro_path(m);
                    if path.len() != 1 {
                        return Err(Error::PathTooDeep(d.macro_display(m)));
                    }
                    let referent = self.lookup(d.strings.get(path[0]))?;
                    let rk = self.comps.kind[referent as usize];
                    if rk != kind {
                        return Err(Error::ExtendKindMismatch { referent: d.macro_display(m), referent_kind: rk.tag().into(), kind: kind.tag().into() });
                    }
                    Some(referent)
                }
                None => None,
            };

            // Virtual multi-cell attributes (a point's coords) bind several slots at once.
            let mut bound_by_virtual = vec![false; kind.prop_defs().len()];
            for (vname, parts) in virtual_attrs(kind) {
                if let Some(a) = d.attr(el, vname) {
                    let m = self.single_macro(a).ok_or_else(|| Error::BadValue { attr: vname.into(), text: self.attr_text(a).unwrap_or_default() })?;
                    let targets = self.resolve_ref(m)?;
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
                let source = match def.from {
                    PropFrom::Attribute => match d.attr(el, def.name) {
                        Some(a) => self.value_source(kind, def.name, d.attr_children(a))?,
                        None => self.inherit_or_default(extend, pi, def.default),
                    },
                    PropFrom::Children => {
                        if d.children(el).iter().all(|&n| self.is_blank(n)) {
                            self.inherit_or_default(extend, pi, def.default)
                        } else {
                            self.value_source(kind, def.name, d.children(el))?
                        }
                    }
                    PropFrom::Derived => self.op_source(el)?,
                };
                let s = self.slot(idx, pi);
                self.sources[s as usize] = source;
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
    fn value_source(&mut self, kind: ComponentKind, attr: &str, nodes: &[NodeId]) -> Result<Source> {
        let d = self.dast;
        let macros: Vec<NodeId> = nodes.iter().copied().filter(|&n| d.kind(n) == NodeKind::Macro).collect();
        let text: String = nodes.iter().filter(|&&n| d.kind(n) == NodeKind::Text).map(|&n| d.str(n)).collect();
        let text = text.trim();
        match (macros.len(), text.is_empty()) {
            (1, true) => {
                let targets = self.resolve_ref(macros[0])?;
                if targets.len() != 1 {
                    return Err(Error::ArityMismatch { kind: kind.tag().into(), prop: attr.into(), expected: 1, got: targets.len() });
                }
                Ok(Source::Alias(targets[0]))
            }
            (0, false) => text.parse::<f64>().map(Source::Literal).map_err(|_| Error::BadValue { attr: attr.into(), text: text.into() }),
            _ => Err(Error::BadValue { attr: attr.into(), text: text.into() }),
        }
    }

    fn op_source(&mut self, el: NodeId) -> Result<Source> {
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
            "negate" => OpSpec::Negate,
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
                        let targets = self.resolve_ref(node)?;
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

    fn lookup(&self, name: &str) -> Result<CompIdx> {
        self.names.get(name.trim()).copied().ok_or_else(|| Error::UnknownName(name.into()))
    }

    /// Slots named by `$name` or `$name.prop`.
    fn resolve_ref(&self, m: NodeId) -> Result<Vec<SlotId>> {
        let d = self.dast;
        let path = d.macro_path(m);
        let name = d.strings.get(path[0]);
        let comp = self.lookup(name)?;
        let kind = self.comps.kind[comp as usize];
        let prop: &str = match path.len() {
            1 => kind.default_prop().ok_or_else(|| Error::NoDefaultProp(name.into()))?,
            2 => d.strings.get(path[1]),
            _ => return Err(Error::PathTooDeep(d.macro_display(m))),
        };
        if let Some(parts) = kind.virtual_prop(prop) {
            return Ok(parts.iter().map(|p| self.slot(comp, kind.prop_index(p).unwrap())).collect());
        }
        let pi = kind.prop_index(prop).ok_or_else(|| Error::UnknownProp { name: name.into(), prop: prop.into() })?;
        Ok(vec![self.slot(comp, pi)])
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

        // Number cells: essential classes first, then derived.
        let mut slot_cell: Vec<CellIdx> = vec![NONE; n];
        let mut cells = Vec::new();
        let mut cell_def_slot: Vec<SlotId> = Vec::new();
        let mut derived_defs = Vec::new();
        for s in 0..n {
            let root = uf.find(s as SlotId) as usize;
            if class_def[root] as usize != s {
                continue; // not the defining slot of its class
            }
            match &self.sources[s] {
                Source::Literal(v) | Source::Default(v) => {
                    slot_cell[root] = cells.len() as CellIdx;
                    cells.push(*v);
                    cell_def_slot.push(s as SlotId);
                }
                Source::Op(..) => derived_defs.push(s),
                _ => unreachable!(),
            }
        }
        let n_essential = cells.len();
        for &s in &derived_defs {
            let root = uf.find(s as SlotId) as usize;
            slot_cell[root] = cells.len() as CellIdx;
            cells.push(f64::NAN);
            cell_def_slot.push(s as SlotId);
        }
        let cell_of = |uf: &mut UnionFind, slot: SlotId| slot_cell[uf.find(slot) as usize];

        let mut instrs = Vec::with_capacity(derived_defs.len());
        for &s in &derived_defs {
            let Source::Op(spec, start, count) = &self.sources[s] else { unreachable!() };
            let inputs = &self.op_inputs[*start as usize..*start as usize + *count as usize];
            let bound: Vec<CellIdx> = inputs.iter().map(|&i| cell_of(&mut uf, i)).collect();
            instrs.push(Instr { out: cell_of(&mut uf, s as SlotId), op: spec.bind(&bound) });
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

        // Lazy labels for cycle errors.
        let slot_comp = self.slot_comp;
        let slot_base = self.slot_base;
        let kinds = self.comps.kind.clone();
        let names: Vec<Option<String>> = self.comps.name.iter().map(|&s| (s != NONE).then(|| self.dast.strings.get(s).trim().to_string())).collect();
        let cell_label = Box::new(move |cell: CellIdx| {
            let slot = cell_def_slot[cell as usize];
            let comp = slot_comp[slot as usize];
            let pi = (slot - slot_base[comp as usize]) as usize;
            let owner = names[comp as usize].clone().unwrap_or_else(|| format!("<{}>#{}", kinds[comp as usize].tag(), comp));
            format!("{owner}.{}", kinds[comp as usize].prop_defs()[pi].name)
        });

        Ok(Unscheduled { cells, n_essential, instrs, comps: self.comps, strings: self.dast.strings.clone(), root: self.root, cell_label })
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
