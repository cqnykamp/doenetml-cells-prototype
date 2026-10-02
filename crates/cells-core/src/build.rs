//! Build a [`Document`] from a DAST: create components, resolve references,
//! merge aliased props into shared cells, and emit the instruction list.
//!
//! Phases:
//! A. Walk the DAST, creating a component per element and a placeholder per
//!    `$ref` child. Allocate one *slot* per single-cell prop.
//! B. Give every slot a source: a literal, a default, an alias of another
//!    slot, or an operator over other slots.
//! C. Union-find over aliases: each class of aliased slots becomes one cell.
//! D. Number the cells (essential first), bind operators, build the program.

use std::collections::HashMap;

use crate::components::{ComponentKind, PropFrom};
use crate::dast::{DastAttribute, DastElement, DastMacro, DastNode, DastRoot};
use crate::document::{CellIdx, Child, CompIdx, Component, Document, Prop};
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
    Op(OpSpec, Vec<SlotId>),
}

/// A built document whose program has not yet been scheduled.
pub struct Unscheduled {
    cells: Vec<f64>,
    n_essential: usize,
    instrs: Vec<Instr>,
    components: Vec<Component>,
    root: CompIdx,
    names: HashMap<String, CompIdx>,
    /// Human-readable owner of each cell, e.g. "p1.x", for error messages.
    cell_label: Vec<String>,
}

impl Unscheduled {
    pub fn schedule(self) -> Result<Document> {
        let n = self.cells.len();
        let program = Program::schedule(self.instrs, n)
            .map_err(|cell| Error::Cycle(self.cell_label[cell as usize].clone()))?;
        Ok(Document::new(self.cells, self.n_essential, program, self.components, self.root, self.names))
    }
}

pub fn build(dast: &DastRoot) -> Result<Unscheduled> {
    let mut b = Builder::default();
    b.phase_a(dast)?;
    b.phase_a2_placeholders()?;
    b.phase_b_sources()?;
    b.phase_cd_cells()
}

#[derive(Default)]
struct Builder<'a> {
    components: Vec<Component>,
    /// slots[comp][prop_index]
    slots: Vec<Vec<SlotId>>,
    sources: Vec<Source>,
    slot_label: Vec<String>,
    names: HashMap<String, CompIdx>,
    /// Explicit elements to process in phase B.
    elements: Vec<(CompIdx, &'a DastElement)>,
    /// `$ref` children awaiting a kind.
    pending: Vec<(CompIdx, &'a DastMacro)>,
    root: CompIdx,
    anon_counter: usize,
}

impl<'a> Builder<'a> {
    // ---- Phase A -----------------------------------------------------------

    fn phase_a(&mut self, dast: &'a DastRoot) -> Result<()> {
        let doc_el = dast.children.iter().find_map(|n| match n {
            DastNode::Element(e) if e.name == "document" => Some(e),
            _ => None,
        });
        match doc_el {
            Some(el) => {
                self.root = self.add_element(el, None)?.unwrap();
            }
            None => {
                // Synthesize a root when the DAST was not normalized.
                self.root = self.new_component(ComponentKind::Document, None, None)?;
                let root = self.root;
                self.add_children(&dast.children, root)?;
            }
        }
        Ok(())
    }

    fn new_component(&mut self, kind: ComponentKind, name: Option<String>, parent: Option<CompIdx>) -> Result<CompIdx> {
        let idx = self.components.len() as CompIdx;
        if let Some(n) = &name
            && self.names.insert(n.clone(), idx).is_some() {
                return Err(Error::DuplicateName(n.clone()));
            }
        self.components.push(Component { kind, name, parent, children: Vec::new(), props: Vec::new() });
        self.slots.push(Vec::new());
        self.allocate_slots(idx);
        Ok(idx)
    }

    fn allocate_slots(&mut self, comp: CompIdx) {
        let c = &self.components[comp as usize];
        let label = c.name.clone().unwrap_or_else(|| {
            self.anon_counter += 1;
            format!("<{}>#{}", c.kind.tag(), self.anon_counter)
        });
        let defs = c.kind.prop_defs();
        let mut slots = Vec::with_capacity(defs.len());
        for def in defs {
            let id = self.sources.len() as SlotId;
            self.sources.push(Source::Unset);
            self.slot_label.push(format!("{label}.{}", def.name));
            slots.push(id);
        }
        self.slots[comp as usize] = slots;
    }

    fn add_element(&mut self, el: &'a DastElement, parent: Option<CompIdx>) -> Result<Option<CompIdx>> {
        if el.name == "_dynamicChildren" {
            return Ok(None);
        }
        let kind = ComponentKind::from_tag(&el.name).ok_or_else(|| Error::UnsupportedTag(el.name.clone()))?;
        let name = el.attributes.get("name").and_then(attr_text).map(|s| s.trim().to_string());
        let idx = self.new_component(kind, name, parent)?;
        self.elements.push((idx, el));
        // A number's children are its value, not rendered children.
        if kind.prop_defs().iter().find(|d| d.from == PropFrom::Children).is_none() {
            self.add_children(&el.children, idx)?;
        }
        Ok(Some(idx))
    }

    fn add_children(&mut self, children: &'a [DastNode], parent: CompIdx) -> Result<()> {
        for node in children {
            match node {
                DastNode::Element(e) => {
                    if let Some(idx) = self.add_element(e, Some(parent))? {
                        self.components[parent as usize].children.push(Child::Component(idx));
                    }
                }
                DastNode::Text(t) => {
                    self.components[parent as usize].children.push(Child::Text(t.value.clone()));
                }
                DastNode::Macro(m) => {
                    // Kind is unknown until names resolve; use Document as a stand-in.
                    let idx = self.new_component(ComponentKind::Document, None, Some(parent))?;
                    self.components[parent as usize].children.push(Child::Component(idx));
                    self.pending.push((idx, m));
                }
                DastNode::Other => {}
            }
        }
        Ok(())
    }

    // ---- Phase A2: give `$ref` children a kind and alias their props -------

    fn phase_a2_placeholders(&mut self) -> Result<()> {
        let pending = std::mem::take(&mut self.pending);
        for (idx, m) in pending {
            match m.path.len() {
                1 => {
                    let referent = self.lookup(&m.path[0].name)?;
                    let kind = self.components[referent as usize].kind;
                    if !kind.copyable() {
                        return Err(Error::UncopyableKind(kind.tag().into()));
                    }
                    self.components[idx as usize].kind = kind;
                    self.allocate_slots(idx);
                    for (i, &slot) in self.slots[idx as usize].clone().iter().enumerate() {
                        self.sources[slot as usize] = Source::Alias(self.slots[referent as usize][i]);
                    }
                }
                2 => {
                    self.components[idx as usize].kind = ComponentKind::Number;
                    self.allocate_slots(idx);
                    let targets = self.resolve_ref(m)?;
                    self.bind_alias(idx, 0, &targets, ComponentKind::Number, "value")?;
                }
                _ => return Err(Error::PathTooDeep(m.display())),
            }
        }
        Ok(())
    }

    // ---- Phase B: sources for explicit elements -----------------------------

    fn phase_b_sources(&mut self) -> Result<()> {
        let elements = std::mem::take(&mut self.elements);
        for (idx, el) in elements {
            let kind = self.components[idx as usize].kind;
            let extend = match el.attributes.get("extend") {
                Some(a) => {
                    let m = single_macro(a).ok_or_else(|| Error::BadValue { attr: "extend".into(), text: attr_text(a).unwrap_or_default() })?;
                    if m.path.len() != 1 {
                        return Err(Error::PathTooDeep(m.display()));
                    }
                    let referent = self.lookup(&m.path[0].name)?;
                    let rk = self.components[referent as usize].kind;
                    if rk != kind {
                        return Err(Error::ExtendKindMismatch { referent: m.display(), referent_kind: rk.tag().into(), kind: kind.tag().into() });
                    }
                    Some(referent)
                }
                None => None,
            };

            // Virtual multi-cell attributes (a point's coords) bind several slots at once.
            let mut bound_by_virtual = vec![false; kind.prop_defs().len()];
            for (vname, parts) in virtual_attrs(kind) {
                if let Some(a) = el.attributes.get(vname) {
                    let m = single_macro(a).ok_or_else(|| Error::BadValue { attr: vname.into(), text: attr_text(a).unwrap_or_default() })?;
                    let targets = self.resolve_ref(m)?;
                    if targets.len() != parts.len() {
                        return Err(Error::ArityMismatch { kind: kind.tag().into(), prop: vname.into(), expected: parts.len(), got: targets.len() });
                    }
                    for (part, &t) in parts.iter().zip(&targets) {
                        let pi = kind.prop_index(part).unwrap();
                        if el.attributes.contains_key(*part) {
                            return Err(Error::BadValue { attr: part.to_string(), text: format!("conflicts with {vname}") });
                        }
                        self.sources[self.slots[idx as usize][pi] as usize] = Source::Alias(t);
                        bound_by_virtual[pi] = true;
                    }
                }
            }

            for (pi, def) in kind.prop_defs().iter().enumerate() {
                if bound_by_virtual[pi] {
                    continue;
                }
                let slot = self.slots[idx as usize][pi];
                let source = match def.from {
                    PropFrom::Attribute => match el.attributes.get(def.name) {
                        Some(a) => self.value_source(idx, pi, def.name, &a.children)?,
                        None => self.inherit_or_default(extend, pi, def.default),
                    },
                    PropFrom::Children => {
                        if el.children.iter().all(is_blank) {
                            self.inherit_or_default(extend, pi, def.default)
                        } else {
                            self.value_source(idx, pi, def.name, &el.children)?
                        }
                    }
                    PropFrom::Derived => self.op_source(el)?,
                };
                self.sources[slot as usize] = source;
            }
        }
        Ok(())
    }

    fn inherit_or_default(&self, extend: Option<CompIdx>, pi: usize, default: f64) -> Source {
        match extend {
            Some(r) => Source::Alias(self.slots[r as usize][pi]),
            None => Source::Default(default),
        }
    }

    /// A literal number or a single reference, as found in an attribute or in
    /// a number's children.
    fn value_source(&mut self, idx: CompIdx, _pi: usize, attr: &str, nodes: &[DastNode]) -> Result<Source> {
        let kind = self.components[idx as usize].kind;
        let macros: Vec<&DastMacro> = nodes.iter().filter_map(|n| if let DastNode::Macro(m) = n { Some(m) } else { None }).collect();
        let text: String = nodes.iter().filter_map(|n| if let DastNode::Text(t) = n { Some(t.value.as_str()) } else { None }).collect();
        let text = text.trim();
        match (macros.len(), text.is_empty()) {
            (1, true) => {
                let targets = self.resolve_ref(macros[0])?;
                if targets.len() != 1 {
                    return Err(Error::ArityMismatch { kind: kind.tag().into(), prop: attr.into(), expected: 1, got: targets.len() });
                }
                Ok(Source::Alias(targets[0]))
            }
            (0, false) => text
                .parse::<f64>()
                .map(Source::Literal)
                .map_err(|_| Error::BadValue { attr: attr.into(), text: text.into() }),
            _ => Err(Error::BadValue { attr: attr.into(), text: text.into() }),
        }
    }

    fn bind_alias(&mut self, idx: CompIdx, pi: usize, targets: &[SlotId], kind: ComponentKind, prop: &str) -> Result<()> {
        if targets.len() != 1 {
            return Err(Error::ArityMismatch { kind: kind.tag().into(), prop: prop.into(), expected: 1, got: targets.len() });
        }
        let slot = self.slots[idx as usize][pi];
        self.sources[slot as usize] = Source::Alias(targets[0]);
        Ok(())
    }

    fn op_source(&mut self, el: &DastElement) -> Result<Source> {
        let kind_text = el.attributes.get("kind").and_then(attr_text).unwrap_or_default();
        let kind_text = kind_text.trim();
        let param = |name: &str| -> Result<f64> {
            let a = el.attributes.get(name).ok_or_else(|| Error::MissingParam { kind: kind_text.into(), attr: name.into() })?;
            attr_text(a)
                .and_then(|t| t.trim().parse::<f64>().ok())
                .ok_or_else(|| Error::BadLiteralParam(name.into()))
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
        let mut inputs = Vec::new();
        if let Some(args) = el.attributes.get("args") {
            for node in &args.children {
                match node {
                    DastNode::Macro(m) => {
                        let targets = self.resolve_ref(m)?;
                        if targets.len() != 1 {
                            return Err(Error::ArityMismatch { kind: "op".into(), prop: "args".into(), expected: 1, got: targets.len() });
                        }
                        inputs.push(targets[0]);
                    }
                    DastNode::Text(t) if t.value.trim().is_empty() => {}
                    DastNode::Text(_) => return Err(Error::LiteralArg),
                    _ => {}
                }
            }
        }
        if inputs.len() != spec.arity() {
            return Err(Error::OpArity { kind: kind_text.into(), expected: spec.arity(), got: inputs.len() });
        }
        Ok(Source::Op(spec, inputs))
    }

    fn lookup(&self, name: &str) -> Result<CompIdx> {
        self.names.get(name).copied().ok_or_else(|| Error::UnknownName(name.into()))
    }

    /// Slots named by `$name` or `$name.prop`.
    fn resolve_ref(&self, m: &DastMacro) -> Result<Vec<SlotId>> {
        let comp = self.lookup(&m.path[0].name)?;
        let kind = self.components[comp as usize].kind;
        let prop: &str = match m.path.len() {
            1 => kind.default_prop().ok_or_else(|| Error::NoDefaultProp(m.path[0].name.clone()))?,
            2 => &m.path[1].name,
            _ => return Err(Error::PathTooDeep(m.display())),
        };
        let slots = &self.slots[comp as usize];
        if let Some(parts) = kind.virtual_prop(prop) {
            return Ok(parts.iter().map(|p| slots[kind.prop_index(p).unwrap()]).collect());
        }
        let pi = kind
            .prop_index(prop)
            .ok_or_else(|| Error::UnknownProp { name: m.path[0].name.clone(), prop: prop.into() })?;
        Ok(vec![slots[pi]])
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
        let mut class_def: Vec<Option<SlotId>> = vec![None; n];
        for (s, src) in self.sources.iter().enumerate() {
            match src {
                Source::Alias(_) => {}
                Source::Unset => unreachable!("slot {} never received a source", self.slot_label[s]),
                _ => {
                    let root = uf.find(s as SlotId) as usize;
                    debug_assert!(class_def[root].is_none(), "two sources in one alias class");
                    class_def[root] = Some(s as SlotId);
                }
            }
        }
        for s in 0..n {
            let root = uf.find(s as SlotId) as usize;
            if class_def[root].is_none() {
                return Err(Error::Cycle(self.slot_label[s].clone()));
            }
        }

        // Number cells: essential classes first, then derived.
        let mut slot_cell: Vec<CellIdx> = vec![u32::MAX; n];
        let mut cells = Vec::new();
        let mut cell_label = Vec::new();
        let mut derived_defs = Vec::new();
        for s in 0..n {
            let root = uf.find(s as SlotId) as usize;
            let def = class_def[root].unwrap() as usize;
            if def != s {
                continue; // not the defining slot of its class
            }
            match &self.sources[s] {
                Source::Literal(v) | Source::Default(v) => {
                    slot_cell[root] = cells.len() as CellIdx;
                    cells.push(*v);
                    cell_label.push(self.slot_label[s].clone());
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
            cell_label.push(self.slot_label[s].clone());
        }
        let cell_of = |uf: &mut UnionFind, slot: SlotId| slot_cell[uf.find(slot) as usize];

        let mut instrs = Vec::with_capacity(derived_defs.len());
        for &s in &derived_defs {
            let Source::Op(spec, inputs) = &self.sources[s] else { unreachable!() };
            let bound: Vec<CellIdx> = inputs.iter().map(|&i| cell_of(&mut uf, i)).collect();
            instrs.push(Instr { out: cell_of(&mut uf, s as SlotId), op: spec.bind(&bound) });
        }

        for (ci, comp) in self.components.iter_mut().enumerate() {
            comp.props = comp
                .kind
                .prop_defs()
                .iter()
                .zip(&self.slots[ci])
                .map(|(def, &slot)| Prop { name: def.name, cells: vec![cell_of(&mut uf, slot)] })
                .collect();
        }

        Ok(Unscheduled {
            cells,
            n_essential,
            instrs,
            components: self.components,
            root: self.root,
            names: self.names,
            cell_label,
        })
    }
}

fn virtual_attrs(kind: ComponentKind) -> Vec<(&'static str, &'static [&'static str])> {
    match kind {
        ComponentKind::Point => vec![("coords", kind.virtual_prop("coords").unwrap())],
        _ => vec![],
    }
}

fn attr_text(a: &DastAttribute) -> Option<String> {
    let mut s = String::new();
    for n in &a.children {
        match n {
            DastNode::Text(t) => s.push_str(&t.value),
            DastNode::Macro(_) => return None,
            _ => {}
        }
    }
    Some(s)
}

fn single_macro(a: &DastAttribute) -> Option<&DastMacro> {
    let mut found = None;
    for n in &a.children {
        match n {
            DastNode::Macro(m) if found.is_none() => found = Some(m),
            DastNode::Macro(_) => return None,
            DastNode::Text(t) if t.value.trim().is_empty() => {}
            DastNode::Text(_) => return None,
            _ => {}
        }
    }
    found
}

fn is_blank(n: &DastNode) -> bool {
    match n {
        DastNode::Text(t) => t.value.trim().is_empty(),
        DastNode::Other => true,
        _ => false,
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
