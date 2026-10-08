//! Choices (plan 6, ADR 0009): `<conditionalContent>` and `<select>`.
//!
//! Each branch (a case or an option) is its own template, so the names
//! inside it are private, as a repeat's are. What the rest of the document
//! may reach is the branch interface: the names every branch declares, each
//! with the same kind in every branch, checked once every element is
//! planned.
//!
//! A select picks its options while the document is built, from the
//! document seed: unchosen options are never expanded. A conditional
//! content's `choice` cell is the first case whose condition holds. If its
//! branches are small the core keeps every branch built, inside a `Case`
//! component whose `active` cell says whether it is shown, and an interface
//! name is a `Choose` over the branches' cells. If not, the choice cell is
//! structural and a change rebuilds the document with only the active case
//! expanded (ADR 0004). Both mechanisms mean the same thing to an author.

use super::*;

/// Largest reactive choice, in template elements across its branches, that
/// stays built rather than rebuilding on a change; a curve counts as
/// `CURVE_WEIGHT` elements. From the plan 6 sweep: a built flip stays under
/// 0.3 ms with 40,000 points in the branches, while a rebuild flip costs a
/// whole-document build (21 ms beside 10,000 points, 150 ms beside
/// 50,000), so size alone rarely argues for rebuilding. What does is the
/// memory of every branch and the work inactive branches still do each
/// tick, which curves dominate (200 sample cells, resampled when a value
/// they read changes). `CELLS_CHOICE=built|rebuild` forces one mechanism.
const BUILT_MAX_WEIGHT: usize = 200_000;
const CURVE_WEIGHT: usize = 50;

/// `Choose` reads the choice cell and one cell per branch; `First` one
/// condition per case. Vector operators read at most `geo::MAX_VEC_IN`.
const MAX_BUILT_BRANCHES: usize = crate::geo::MAX_VEC_IN - 1;

impl<'a> Compiler<'a> {
    /// Give a choice element its branch templates. `<group rendered="c">`
    /// is a conditional content with one case.
    pub(super) fn add_choice(&mut self, t: TemplateId, e: ElemId, el: NodeId) -> Result<()> {
        let d = self.c.dast;
        let reactive = self.c.templates[t].elems[e].kind == ComponentKind::ConditionalContent;
        let tag = d.str(el).to_string();
        // (condition attribute, body nodes, weight) per branch.
        let mut branches: Vec<(Option<u32>, Vec<NodeId>, f64)> = Vec::new();
        let is_branch = |n: NodeId| d.kind(n) == NodeKind::Element && matches!(d.str(n), "case" | "else" | "option");
        if tag == "group" {
            branches.push((d.attr(el, "rendered"), d.children(el).to_vec(), 1.0));
        } else if reactive && d.attr(el, "condition").is_some() && !d.children(el).iter().any(|&n| is_branch(n)) {
            branches.push((d.attr(el, "condition"), d.children(el).to_vec(), 1.0));
        } else {
            let want = if reactive { "<case> or <else>" } else { "<option>" };
            for &n in d.children(el) {
                match d.kind(n) {
                    NodeKind::Text if d.str(n).trim().is_empty() => {}
                    NodeKind::Other => {}
                    NodeKind::Element if reactive && matches!(d.str(n), "case" | "else") => {
                        branches.push((d.attr(n, "condition"), self.branch_body(n), 1.0));
                    }
                    NodeKind::Element if !reactive && d.str(n) == "option" => {
                        let weight = match d.attr(n, "selectWeight") {
                            Some(a) => self.literal_attr(a, "selectWeight")?,
                            None => 1.0,
                        };
                        branches.push((None, self.branch_body(n), weight));
                    }
                    _ => return Err(Error::Unsupported(format!("the children of <{tag}> must be {want} elements"))),
                }
            }
        }
        if branches.is_empty() {
            return Err(Error::Unsupported(format!("<{tag}> with no branches")));
        }
        let (num_to_select, with_replacement) = if reactive {
            (1, false)
        } else {
            let num = match d.attr(el, "numToSelect") {
                Some(a) => self.literal_attr(a, "numToSelect")?,
                None => 1.0,
            };
            if num < 0.0 || num.fract() != 0.0 {
                return Err(Error::BadValue { attr: "numToSelect".into(), text: num.to_string() });
            }
            (num as u32, self.attr_flag(el, "withReplacement"))
        };
        let mut templates = Vec::with_capacity(branches.len());
        for (_, nodes, _) in &branches {
            let sub = self.c.templates.len();
            self.c.templates.push(Template { parent: Some((t, e)), ..Default::default() });
            let kids = self.add_children(sub, None, ROOT_SCOPE, nodes)?;
            self.c.templates[sub].children = kids;
            templates.push(sub);
        }
        let id = self.c.choices.len();
        self.c.choices.push(ChoiceDef {
            at: (t, e),
            reactive,
            branches: templates,
            conditions: branches.iter().map(|b| b.0).collect(),
            num_to_select,
            with_replacement,
            weights: branches.iter().map(|b| b.2).collect(),
            iface: HashMap::new(),
            used: Vec::new(),
            built: false,
        });
        self.c.templates[t].elems[e].body = Body::Choice(id);
        Ok(())
    }

    /// A case's or option's content. The normalizer wraps a case's children
    /// in a `<group>`; that group is the branch itself, not content.
    fn branch_body(&self, n: NodeId) -> Vec<NodeId> {
        let d = self.c.dast;
        let content: Vec<NodeId> = d.children(n).iter().copied().filter(|&c| !self.is_blank(c)).collect();
        match content.as_slice() {
            [g] if d.kind(*g) == NodeKind::Element && d.str(*g) == "group" && d.attrs(*g).is_empty() => d.children(*g).to_vec(),
            _ => d.children(n).to_vec(),
        }
    }

    /// A literal number attribute of a choice; a reference is banned, since
    /// a load-time choice depends only on the seed and literals (ADR 0009).
    fn literal_attr(&self, a: u32, name: &str) -> Result<f64> {
        let text = self.attr_text(a).ok_or_else(|| Error::Banned(format!("'{name}' must be a literal number: a load-time choice depends only on the document seed and literals")))?;
        text.trim().parse::<f64>().map_err(|_| Error::BadValue { attr: name.into(), text })
    }

    /// A reactive choice's props: a hidden slot per case condition (an else
    /// is a constant 1) and `choice`, the first of them that holds.
    pub(super) fn plan_choice(&mut self, t: TemplateId, e: ElemId) -> Result<()> {
        let Body::Choice(cid) = self.c.templates[t].elems[e].body else { unreachable!() };
        if !self.c.choices[cid].reactive {
            self.c.templates[t].elems[e].props = Vec::new();
            return Ok(());
        }
        let d = self.c.dast;
        let mut plan = ElemPlan::new(1);
        let mut conds = Vec::new();
        for c in self.c.choices[cid].conditions.clone() {
            conds.push(match c {
                Some(a) => self.plan_condition(t, e, &mut plan, d.attr_children(a))?,
                None => plan.hidden(SourcePlan::Fixed(1.0)),
            });
        }
        if conds.len() > crate::geo::MAX_VEC_IN {
            return Err(Error::Unsupported(format!("a <conditionalContent> with more than {} cases", crate::geo::MAX_VEC_IN)));
        }
        // After the conditions, so creation order stays an evaluation order.
        let first = plan.hidden(SourcePlan::vector(VecOp::First { n: conds.len() as u8 }, conds));
        plan.set(0, SourcePlan::own(first));
        self.c.templates[t].elems[e].props = plan.finish();
        Ok(())
    }

    /// A condition as hidden slots of element `e`; returns the slot holding
    /// 1 or 0. Comparisons (`< <= > >= = !=`), `and`/`&&`, `or`/`||`,
    /// `not`/`!`, parentheses, `true`, `false`, and numeric math operands;
    /// a bare operand holds when it is nonzero.
    fn plan_condition(&mut self, t: TemplateId, e: ElemId, plan: &mut ElemPlan, nodes: &[NodeId]) -> Result<u8> {
        let (toks, text) = self.math_tokens(t, e, nodes)?;
        let mut p = CondParser { toks: &toks, pos: 0, text: &text };
        let slot = p.or(self, plan)?;
        if p.pos != toks.len() {
            return Err(p.err(format!("unexpected {:?}", toks[p.pos])));
        }
        Ok(slot)
    }

    /// The branch interface of every choice, once every element is planned
    /// (a tuple-valued `<math>` only becomes a point when planned), then the
    /// check that every interface name a reference uses is in it, and the
    /// mechanism of each reactive choice.
    pub(super) fn finish_choices(&mut self) -> Result<()> {
        let force = std::env::var("CELLS_CHOICE").ok();
        let sizes = self.template_sizes();
        for cid in 0..self.c.choices.len() {
            let def = &self.c.choices[cid];
            // A conditional content without an else has an implicit empty
            // branch, so nothing in it is reachable from outside.
            let empty_branch = def.reactive && def.conditions.last().is_some_and(|c| c.is_some());
            let mut iface = HashMap::new();
            if !empty_branch {
                let first = &self.c.templates[def.branches[0]];
                for ((at, name), elems) in &first.names {
                    let [e0] = elems.as_slice() else { continue };
                    if *at != ROOT_SCOPE {
                        continue;
                    }
                    let kind = first.elems[*e0].kind;
                    let per_branch: Option<Vec<ElemId>> = def
                        .branches
                        .iter()
                        .map(|&b| match self.c.templates[b].names.get(&(ROOT_SCOPE, name.clone())).map(Vec::as_slice) {
                            Some([x]) if self.c.templates[b].elems[*x].kind == kind => Some(*x),
                            _ => None,
                        })
                        .collect();
                    if let Some(per_branch) = per_branch {
                        iface.insert(name.clone(), (kind, per_branch));
                    }
                }
            }
            for name in &def.used {
                match iface.get(name) {
                    Some((kind, _)) if !kind.copyable() || matches!(kind, ComponentKind::Function | ComponentKind::Derivative | ComponentKind::Answer) => {
                        return Err(Error::Unsupported(format!("a <{}> in a branch interface ('{name}')", kind.tag())));
                    }
                    Some(_) => {}
                    None => return Err(self.not_in_interface(cid, name, empty_branch)),
                }
            }
            let n_branches = def.branches.len();
            // `CELLS_CHOICE`, else the prototype-only `_mechanism` attribute,
            // forces a mechanism (for measuring and testing both).
            let node = self.c.templates[def.at.0].elems[def.at.1].node;
            let attr = self.c.dast.attr(node, "_mechanism").and_then(|a| self.attr_text(a));
            let built = def.reactive
                && n_branches <= MAX_BUILT_BRANCHES
                && match force.as_deref().or(attr.as_deref().map(str::trim)) {
                    Some("built") => true,
                    Some("rebuild") => false,
                    _ => def.branches.iter().map(|&b| sizes[b]).sum::<usize>() <= BUILT_MAX_WEIGHT,
                };
            let def = &mut self.c.choices[cid];
            def.iface = iface;
            def.built = built;
        }
        Ok(())
    }

    /// Why `name` is not in a choice's branch interface.
    fn not_in_interface(&self, cid: ChoiceId, name: &str, empty_branch: bool) -> Error {
        let def = &self.c.choices[cid];
        let choice = self.elem_label(def.at.0, def.at.1);
        let what = if def.reactive { "case" } else { "option" };
        let reason = if empty_branch {
            "it has no <else>, so no branch is active when every condition fails".to_string()
        } else {
            let mut first: Option<(usize, ComponentKind)> = None;
            let mut reason = String::new();
            for (b, &tpl) in def.branches.iter().enumerate() {
                match self.c.templates[tpl].names.get(&(ROOT_SCOPE, name.to_string())).map(Vec::as_slice) {
                    Some([x]) => {
                        let kind = self.c.templates[tpl].elems[*x].kind;
                        match first {
                            None => first = Some((b, kind)),
                            Some((b0, k0)) if k0 != kind => {
                                reason = format!("'{name}' is a <{}> in {what} {} but a <{}> in {what} {}", k0.tag(), b0 + 1, kind.tag(), b + 1);
                                break;
                            }
                            _ => {}
                        }
                    }
                    Some(_) => {
                        reason = format!("{what} {} has several components named '{name}'", b + 1);
                        break;
                    }
                    None => {
                        reason = format!("{what} {} has no '{name}'", b + 1);
                        break;
                    }
                }
            }
            reason
        };
        Error::NotInInterface { choice, name: name.to_string(), reason }
    }

    /// Weight per template (elements, a curve counting `CURVE_WEIGHT`),
    /// nested templates included. A nested template is created after its
    /// parent, so one backward pass sums them.
    fn template_sizes(&self) -> Vec<usize> {
        let weight = |e: &Elem| if matches!(e.kind, ComponentKind::Function | ComponentKind::Derivative) { CURVE_WEIGHT } else { 1 };
        let mut sizes: Vec<usize> = self.c.templates.iter().map(|t| t.elems.iter().map(weight).sum()).collect();
        for i in (1..self.c.templates.len()).rev() {
            if let Some((p, _)) = self.c.templates[i].parent {
                sizes[p] += sizes[i];
            }
        }
        sizes
    }

    /// The step for interface name `name` of choice `cid`, and the element
    /// of the first branch that has it, which stands for every branch while
    /// the rest of the path is planned (the interface check makes their
    /// kinds equal).
    pub(super) fn iface_step(&mut self, cid: ChoiceId, name: &str, display: &str) -> Result<(Step, TemplateId, ElemId)> {
        let name = name.trim();
        let def = &self.c.choices[cid];
        let found = def.branches.iter().find_map(|&b| match self.c.templates[b].names.get(&(ROOT_SCOPE, name.to_string())).map(Vec::as_slice) {
            Some([x]) => Some((b, *x)),
            _ => None,
        });
        let Some((tpl, elem)) = found else {
            return Err(Error::UnknownName(display.to_string()));
        };
        let def = &mut self.c.choices[cid];
        let u = match def.used.iter().position(|n| n == name) {
            Some(u) => u,
            None => {
                def.used.push(name.to_string());
                def.used.len() - 1
            }
        };
        Ok((Step::Iface(cid, u as u32), tpl, elem))
    }
}

/// Recursive descent over a condition's tokens. Operands are numeric math,
/// planned as hidden slots of the choice element.
struct CondParser<'t> {
    toks: &'t [Token],
    pos: usize,
    text: &'t str,
}

impl CondParser<'_> {
    fn err(&self, reason: String) -> Error {
        Error::BadMath { text: self.text.to_string(), reason }
    }

    fn peek(&self, k: usize) -> Option<&Token> {
        self.toks.get(self.pos + k)
    }

    fn is_word(&self, w: &str) -> bool {
        matches!(self.peek(0), Some(Token::Ident(s)) if s == w)
    }

    fn is_op(&self, k: usize, c: char) -> bool {
        matches!(self.peek(k), Some(Token::Op(x)) if *x == c)
    }

    fn or(&mut self, cp: &mut Compiler, plan: &mut ElemPlan) -> Result<u8> {
        let mut lhs = self.and(cp, plan)?;
        loop {
            if self.is_word("or") {
                self.pos += 1;
            } else if self.is_op(0, '|') {
                self.pos += if self.is_op(1, '|') { 2 } else { 1 };
            } else {
                return Ok(lhs);
            }
            let rhs = self.and(cp, plan)?;
            lhs = plan.hidden(SourcePlan::computed(OpSpec::Max, vec![lhs, rhs]));
        }
    }

    fn and(&mut self, cp: &mut Compiler, plan: &mut ElemPlan) -> Result<u8> {
        let mut lhs = self.not(cp, plan)?;
        loop {
            if self.is_word("and") {
                self.pos += 1;
            } else if self.is_op(0, '&') {
                self.pos += if self.is_op(1, '&') { 2 } else { 1 };
            } else {
                return Ok(lhs);
            }
            let rhs = self.not(cp, plan)?;
            lhs = plan.hidden(SourcePlan::computed(OpSpec::Min, vec![lhs, rhs]));
        }
    }

    fn not(&mut self, cp: &mut Compiler, plan: &mut ElemPlan) -> Result<u8> {
        if self.is_word("not") || (self.is_op(0, '!') && !matches!(self.peek(1), Some(Token::Eq))) {
            self.pos += 1;
            let a = self.not(cp, plan)?;
            return Ok(plan.hidden(SourcePlan::computed(OpSpec::Not, vec![a])));
        }
        self.cmp(cp, plan)
    }

    /// A parenthesized condition, or a comparison of two operands, or a
    /// lone operand.
    fn cmp(&mut self, cp: &mut Compiler, plan: &mut ElemPlan) -> Result<u8> {
        if let Some(Token::LParen) = self.peek(0) {
            let close = self.matching(self.pos)?;
            if self.toks[self.pos + 1..close].iter().any(is_condition_token) {
                self.pos += 1;
                let inner = self.or(cp, plan)?;
                if self.pos != close {
                    return Err(self.err("missing ')'".into()));
                }
                self.pos += 1;
                return Ok(inner);
            }
        }
        let a = self.operand(cp, plan)?;
        let rel = match (self.peek(0), self.peek(1)) {
            (Some(Token::Op('<')), Some(Token::Eq)) => Some(("<=", 2)),
            (Some(Token::Op('>')), Some(Token::Eq)) => Some((">=", 2)),
            (Some(Token::Op('!')), Some(Token::Eq)) => Some(("!=", 2)),
            (Some(Token::Eq), Some(Token::Eq)) => Some(("=", 2)),
            (Some(Token::Op('<')), _) => Some(("<", 1)),
            (Some(Token::Op('>')), _) => Some((">", 1)),
            (Some(Token::Eq), _) => Some(("=", 1)),
            _ => None,
        };
        let Some((rel, width)) = rel else {
            return Ok(plan.hidden(SourcePlan::computed(OpSpec::Truthy, vec![a])));
        };
        self.pos += width;
        let b = self.operand(cp, plan)?;
        Ok(match rel {
            "<" => plan.hidden(SourcePlan::computed(OpSpec::Lt, vec![a, b])),
            "<=" => plan.hidden(SourcePlan::computed(OpSpec::Le, vec![a, b])),
            ">" => plan.hidden(SourcePlan::computed(OpSpec::Lt, vec![b, a])),
            ">=" => plan.hidden(SourcePlan::computed(OpSpec::Le, vec![b, a])),
            "=" => plan.hidden(SourcePlan::computed(OpSpec::Eq, vec![a, b])),
            _ => {
                let eq = plan.hidden(SourcePlan::computed(OpSpec::Eq, vec![a, b]));
                plan.hidden(SourcePlan::computed(OpSpec::Not, vec![eq]))
            }
        })
    }

    /// Math up to the next comparison or connective outside parentheses.
    fn operand(&mut self, cp: &mut Compiler, plan: &mut ElemPlan) -> Result<u8> {
        let start = self.pos;
        let mut depth = 0i32;
        while let Some(t) = self.peek(0) {
            match t {
                Token::LParen => depth += 1,
                Token::RParen if depth == 0 => break,
                Token::RParen => depth -= 1,
                _ if depth == 0 && is_condition_token(t) => break,
                _ => {}
            }
            self.pos += 1;
        }
        let toks = &self.toks[start..self.pos];
        let source = match toks {
            [] => return Err(self.err("expected a value".into())),
            [Token::Ident(w)] if w == "true" => SourcePlan::Fixed(1.0),
            [Token::Ident(w)] if w == "false" => SourcePlan::Fixed(0.0),
            _ => {
                let id = Parser::parse(toks, &mut cp.c.arena).map_err(|reason| self.err(reason))?;
                match cp.c.arena.get(id) {
                    // A constant in a condition is not state.
                    Expr::Num(v) => SourcePlan::Fixed(*v),
                    Expr::Cell(p) => SourcePlan::reference(*p as PlanId),
                    _ => SourcePlan::Math(id),
                }
            }
        };
        Ok(plan.hidden(source))
    }

    fn matching(&self, open: usize) -> Result<usize> {
        let mut depth = 0i32;
        for (i, t) in self.toks.iter().enumerate().skip(open) {
            match t {
                Token::LParen => depth += 1,
                Token::RParen => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(i);
                    }
                }
                _ => {}
            }
        }
        Err(self.err("missing ')'".into()))
    }
}

/// Tokens that only appear in conditions, never in their math operands.
fn is_condition_token(t: &Token) -> bool {
    match t {
        Token::Eq => true,
        Token::Op(c) => matches!(c, '<' | '>' | '!' | '&' | '|'),
        Token::Ident(w) => matches!(w.as_str(), "and" | "or" | "not"),
        _ => false,
    }
}

impl<'c, 'a> Builder<'c, 'a> {
    /// Expand choice `cid` at component `comp`; returns its children.
    pub(super) fn expand_choice(&mut self, cid: ChoiceId, node: NodeId, scope: ScopeId, comp: CompIdx) -> Result<Vec<u32>> {
        let c: &'c Compiled<'a> = self.c;
        let def = &c.choices[cid];
        let mut inst = ChoiceInst { def: cid, comp, scopes: Vec::new(), branch_of: Vec::new(), iface_comps: Vec::new() };
        let mut kids = Vec::new();
        if !def.reactive {
            for (j, b) in self.pick(def, scope, node)?.into_iter().enumerate() {
                let s = self.scope_for(scope, node, j as u32 + 1);
                self.enter_scope(s, def.branches[b]);
                kids.extend(self.expand(def.branches[b], s, comp)?);
                inst.scopes.push(s);
                inst.branch_of.push(b);
            }
        } else {
            let n = def.branches.len();
            let active = if def.built {
                (1..=n).collect()
            } else {
                match self.prior.counts.get(&(scope, node)).copied().unwrap_or(0) as usize {
                    0 => Vec::new(),
                    k => vec![k.min(n)],
                }
            };
            let choice = self.slot(comp, 0);
            for &k in &active {
                let s = self.scope_for(scope, node, k as u32);
                let case = self.new_component(ComponentKind::Case, NONE, comp, NONE, scope, 1);
                let pos = self.anon_slot(Source::Fixed(k as f64));
                let start = self.op_inputs.len() as u32;
                self.op_inputs.extend_from_slice(&[choice, pos]);
                let active_slot = self.slot(case, 0);
                self.sources[active_slot as usize] = Source::Op(OpSpec::Eq, start, 2);
                self.enter_scope(s, def.branches[k - 1]);
                let these = self.expand(def.branches[k - 1], s, case)?;
                self.set_children(case, &these);
                for &x in &these {
                    if x & TEXT_BIT == 0 {
                        self.comps.parent[x as usize] = case;
                    }
                }
                kids.push(case);
                inst.scopes.push(s);
                inst.branch_of.push(k - 1);
            }
            if def.built {
                for name in &def.used {
                    let kind = def.iface[name].0;
                    // Unnamed: a test path `cc.x` finds the active case's `x`.
                    let ic = self.new_component(kind, NONE, comp, NONE, scope, kind.prop_defs().len());
                    inst.iface_comps.push(ic);
                }
            } else {
                let k = active.first().copied().unwrap_or(0) as u32;
                self.comp_repeat[comp as usize] = self.repeats.len() as u32;
                self.repeats.push(Repeat { comp, node, scope, iter_scopes: inst.scopes.clone(), n: k });
                self.counts_used.push(k);
            }
        }
        self.comp_choice.insert(comp, self.choice_insts.len());
        self.choice_insts.push(inst);
        Ok(kids)
    }

    /// A select's picks: option indices drawn from a stream that depends on
    /// the document seed and the select's position (its element and the
    /// iteration chain it sits in), so it draws the same way in every build.
    fn pick(&self, def: &ChoiceDef, scope: ScopeId, node: NodeId) -> Result<Vec<usize>> {
        let n = def.branches.len();
        if !def.with_replacement && def.num_to_select as usize > n {
            return Err(Error::Unsupported(format!("numToSelect={} is more than the {n} options of a <select> without replacement", def.num_to_select)));
        }
        let mut h = splitmix(self.prior.seed ^ 0x5eed_5e1e_c7ed_0001);
        h = splitmix(h ^ node as u64);
        let mut s = scope;
        while s != 0 {
            let (parent, n, k) = self.scopes[s as usize];
            h = splitmix(h ^ ((n as u64) << 32 | k as u64));
            s = parent;
        }
        let mut weights = def.weights.clone();
        let mut picks = Vec::with_capacity(def.num_to_select as usize);
        for _ in 0..def.num_to_select {
            h = splitmix(h);
            let total: f64 = weights.iter().sum();
            if total <= 0.0 {
                return Err(Error::Unsupported("a <select> whose remaining options all have weight 0".into()));
            }
            let mut r = (h >> 11) as f64 / (1u64 << 53) as f64 * total;
            let mut b = n - 1;
            for (i, &w) in weights.iter().enumerate() {
                if r < w {
                    b = i;
                    break;
                }
                r -= w;
            }
            picks.push(b);
            if !def.with_replacement {
                weights[b] = 0.0;
            }
        }
        Ok(picks)
    }

    /// Sources of the `Choose` components of built reactive choices: each
    /// public prop chooses among the branches' cells by the choice cell.
    /// Runs before instance sources, so a symbolic interface math is known
    /// to be one when its copies are planned.
    pub(super) fn choice_sources(&mut self) {
        let c: &'c Compiled<'a> = self.c;
        for i in 0..self.choice_insts.len() {
            if self.choice_insts[i].iface_comps.is_empty() {
                continue;
            }
            let inst = self.choice_insts[i].clone();
            let def = &c.choices[inst.def];
            let choice = self.slot(inst.comp, 0);
            for (u, &ic) in inst.iface_comps.iter().enumerate() {
                let (kind, elems) = &def.iface[&def.used[u]];
                let members: Vec<CompIdx> = inst.scopes.iter().zip(&inst.branch_of).map(|(&s, &b)| self.scope_comps[s as usize][elems[b]]).collect();
                let symbolic = members.iter().any(|&m| self.is_symbolic(m));
                self.symbolic.resize(self.comps.len(), 0);
                self.symbolic[ic as usize] = if symbolic { 2 } else { 1 };
                let n = members.len() as u8;
                for pi in 0..kind.prop_defs().len() {
                    let start = self.op_inputs.len() as u32;
                    self.op_inputs.push(choice);
                    for &m in &members {
                        let s = self.slot(m, pi);
                        self.op_inputs.push(s);
                    }
                    let s = self.slot(ic, pi);
                    self.sources[s as usize] = Source::Vec(VecOp::Choose { n }, start, n + 1);
                }
                if symbolic && let Some(pi) = kind.prop_index("expr") {
                    let s = self.slot(ic, pi);
                    self.math_slots.push(s);
                }
            }
        }
    }

    /// Where interface name `used[u]` of choice `cid` lands from `cur`.
    pub(super) fn resolve_iface(&self, cid: ChoiceId, u: u32, cur: Resolved) -> (Resolved, ScopeId) {
        let def = &self.c.choices[cid];
        let elems = &def.iface[&def.used[u as usize]].1;
        let in_branch = |inst: &ChoiceInst, j: usize| {
            let s = inst.scopes[j];
            (Resolved::Comp(self.scope_comps[s as usize][elems[inst.branch_of[j]]]), s)
        };
        match cur {
            Resolved::Iter(c, s) => {
                let inst = &self.choice_insts[self.comp_choice[&c]];
                let j = inst.scopes.iter().position(|&x| x == s).expect("a pick of this select");
                in_branch(inst, j)
            }
            Resolved::Comp(c) => {
                let inst = &self.choice_insts[self.comp_choice[&c]];
                if def.built {
                    (Resolved::Comp(inst.iface_comps[u as usize]), self.comps.scope[c as usize])
                } else if inst.scopes.is_empty() {
                    (Resolved::Missing, 0)
                } else {
                    in_branch(inst, 0)
                }
            }
            Resolved::Missing => (Resolved::Missing, 0),
        }
    }
}

fn splitmix(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}
