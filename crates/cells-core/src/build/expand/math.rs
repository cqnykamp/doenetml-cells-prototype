//! Math at expansion: a math's source per instance, numeric (lowered to
//! operators over slots) or symbolic (an `Instantiate` over its leaves),
//! and whether a component's `expr` is a math cell at all.

use super::*;

impl<'c, 'a> Builder<'c, 'a> {
    /// A math cell from a template's math text: a fixed handle when it has
    /// no cell leaves, an alias when it is one math leaf and nothing else,
    /// else an `Instantiate` over its leaves (each a number or a math cell).
    pub(in crate::build) fn sym_source(
        &mut self,
        expr: ExprId,
        post: Post,
        scope: ScopeId,
    ) -> Result<Source> {
        let text = self
            .compiled
            .sym_text
            .get(&expr)
            .ok_or_else(|| Error::BadMath {
                text: format!("{:?}", self.compiled.arena.get(expr)),
                reason: "no math text recorded".into(),
            })?;
        let tree = cells_sym::parse::parse(text).map_err(|reason| Error::BadMath {
            text: text.clone(),
            reason,
        })?;
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
        u8::try_from(leaves.len()).map_err(|_| Error::BadMath {
            text: text.clone(),
            reason: "more than 255 references".into(),
        })?;
        Ok(self.op_source(
            OpSpec::Sym(SymKind::Instantiate { template, post }),
            &leaves,
        ))
    }

    /// Rebind a parsed template's `#plan` leaves to slots: a math leaf where
    /// the reference names an expression, else a numeric leaf. Distinct leaf
    /// slots are appended to `leaves`.
    fn bind_leaves(&mut self, t: &Tree, scope: ScopeId, leaves: &mut Vec<SlotId>) -> Result<Tree> {
        let mut kids = |ts: &[Tree], this: &mut Self| {
            ts.iter()
                .map(|k| this.bind_leaves(k, scope, leaves))
                .collect::<Result<Vec<_>>>()
        };
        Ok(match t {
            Tree::Cell { cell: plan, .. } => {
                let (slot, math) = self.leaf_slot(*plan as RefId, scope)?;
                if !leaves.contains(&slot) {
                    leaves.push(slot);
                }
                Tree::Cell { cell: slot, math }
            }
            Tree::Num(_) | Tree::Sym(_) => t.clone(),
            Tree::Add(ts) => Tree::Add(kids(ts, self)?),
            Tree::Mul(ts) => Tree::Mul(kids(ts, self)?),
            Tree::Sub(a, b) => Tree::Sub(
                Box::new(self.bind_leaves(a, scope, leaves)?),
                Box::new(self.bind_leaves(b, scope, leaves)?),
            ),
            Tree::Div(a, b) => Tree::Div(
                Box::new(self.bind_leaves(a, scope, leaves)?),
                Box::new(self.bind_leaves(b, scope, leaves)?),
            ),
            Tree::Pow(a, b) => Tree::Pow(
                Box::new(self.bind_leaves(a, scope, leaves)?),
                Box::new(self.bind_leaves(b, scope, leaves)?),
            ),
            Tree::Neg(a) => Tree::Neg(Box::new(self.bind_leaves(a, scope, leaves)?)),
            Tree::Apply(f, a) => {
                Tree::Apply(f.clone(), Box::new(self.bind_leaves(a, scope, leaves)?))
            }
        })
    }

    /// The slot a `$ref` inside math names, and whether it holds an
    /// expression (then the slot is the referent's math cell).
    fn leaf_slot(&mut self, plan: RefId, scope: ScopeId) -> Result<(SlotId, bool)> {
        let display = &self.compiled.refs[plan].display;
        let slot = self.resolve_one(plan, scope).map_err(|e| match e {
            Error::ArityMismatch { .. } => Error::BadMath {
                text: display.clone(),
                reason: "a reference inside math must name one cell".into(),
            },
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
        let kind = self.components.kind[comp as usize];
        let pi = (slot - self.slot_base[comp as usize]) as usize;
        let name = kind.prop_defs().get(pi)?.name;
        match (kind, name) {
            (ComponentKind::Answer, "response" | "correct" | "submitted") => Some(slot),
            (
                ComponentKind::Math
                | ComponentKind::MathInput
                | ComponentKind::Function
                | ComponentKind::Derivative,
                "expr" | "value",
            ) => self
                .is_symbolic(comp)
                .then(|| self.slot(comp, kind.prop_index("expr").unwrap())),
            _ => None,
        }
    }

    /// Whether a component's `expr` is a math cell: a function, an unbound
    /// mathInput, a math with a free symbol or a reference to an
    /// expression. A copy is symbolic when its referent is. A reference
    /// cycle counts as numeric (the cycle is reported later).
    pub(in crate::build) fn is_symbolic(&mut self, comp: CompIdx) -> bool {
        if self.math_mode.len() < self.components.len() {
            self.math_mode
                .resize(self.components.len(), MathMode::Unknown);
        }
        match self.math_mode[comp as usize] {
            MathMode::Numeric | MathMode::Deciding => return false,
            MathMode::Symbolic => return true,
            MathMode::Unknown => {}
        }
        self.math_mode[comp as usize] = MathMode::Deciding;
        let kind = self.components.kind[comp as usize];
        let inst = self.comp_instance[comp as usize];
        let yes = match kind {
            ComponentKind::Function | ComponentKind::Derivative => true,
            // A copy of a math (its `expr`) or a bound mathInput (its `value`).
            ComponentKind::Math | ComponentKind::MathInput if inst == NONE => {
                match self.sources.get(self.slot(
                    comp,
                    if kind == ComponentKind::Math {
                        prop::math::EXPR
                    } else {
                        prop::math_input::VALUE
                    },
                ) as usize)
                {
                    Some(Source::Alias(t)) => {
                        let referent = self.slot_comp[*t as usize];
                        referent != NONE && self.is_symbolic(referent)
                    }
                    _ => false,
                }
            }
            ComponentKind::MathInput => {
                let i = self.instances[inst as usize];
                matches!(
                    self.compiled.templates[i.template].elems[i.elem]
                        .props
                        .get(1),
                    Some(SourcePlan::MathEssential(_))
                )
            }
            ComponentKind::Math => {
                let i = self.instances[inst as usize];
                match self.compiled.templates[i.template].elems[i.elem]
                    .props
                    .first()
                {
                    Some(SourcePlan::MathHandle(id, _)) => {
                        let id = *id;
                        let mut syms = Vec::new();
                        self.compiled.arena.symbols(id, &mut syms);
                        let mut leaves = Vec::new();
                        self.compiled.arena.cell_leaves(id, &mut leaves);
                        !syms.is_empty()
                            || leaves.into_iter().any(|p| {
                                self.resolve_one(p as RefId, i.scope)
                                    .is_ok_and(|s| self.math_target(s).is_some())
                            })
                    }
                    _ => false,
                }
            }
            _ => false,
        };
        self.math_mode[comp as usize] = if yes {
            MathMode::Symbolic
        } else {
            MathMode::Numeric
        };
        yes
    }

    /// Copy an expression template into the document's arena with its plan
    /// leaves resolved to slots in `scope`.
    pub(in crate::build) fn instantiate_expr(
        &mut self,
        id: ExprId,
        scope: ScopeId,
    ) -> Result<ExprId> {
        let e = self.compiled.arena.get(id).clone();
        let out = match e {
            Expr::Num(v) => Expr::Num(v),
            Expr::Sym(s) => Expr::Sym(s),
            Expr::Cell(plan) => {
                let display = &self.compiled.refs[plan as usize].display;
                let slot = self
                    .resolve_one(plan as RefId, scope)
                    .map_err(|e| match e {
                        Error::ArityMismatch { .. } => Error::BadMath {
                            text: display.clone(),
                            reason: "a reference inside math must name one cell".into(),
                        },
                        other => other,
                    })?;
                Expr::Cell(slot)
            }
            Expr::Add(a, b)
            | Expr::Sub(a, b)
            | Expr::Mul(a, b)
            | Expr::Div(a, b)
            | Expr::Pow(a, b) => e.with_operands(
                self.instantiate_expr(a, scope)?,
                self.instantiate_expr(b, scope)?,
            ),
            Expr::Neg(a) => Expr::Neg(self.instantiate_expr(a, scope)?),
        };
        Ok(self.arena.push(out))
    }

    /// Lower a numeric expression to operator slots. Literals fold into
    /// `Scale`/`Offset` parameters where an operator has one; otherwise they
    /// become fixed cells.
    pub(in crate::build) fn lower(&mut self, id: ExprId) -> SlotId {
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
            // Commutative: a literal on either side folds into the operator.
            Expr::Add(a, b) | Expr::Mul(a, b) => {
                let (with_k, both): (fn(f64) -> OpSpec, _) = match e {
                    Expr::Add(..) => (|k| OpSpec::Offset { k }, OpSpec::Add),
                    _ => (|k| OpSpec::Scale { k }, OpSpec::Mul),
                };
                match num(&self.arena, b)
                    .map(|k| (a, k))
                    .or_else(|| num(&self.arena, a).map(|k| (b, k)))
                {
                    Some((x, k)) => {
                        let x = self.lower(x);
                        self.op_slot(with_k(k), &[x])
                    }
                    None => {
                        let (a, b) = (self.lower(a), self.lower(b));
                        self.op_slot(both, &[a, b])
                    }
                }
            }
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
}
