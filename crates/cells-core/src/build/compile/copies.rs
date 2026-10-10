//! Copies: `extend` with a prop path, container copies whose children are
//! clones, and the clone elements themselves. The merged-attribute rule for
//! a planned kind with overrides is in `geometry/mod.rs` (`plan_geo`).

use super::*;

impl<'a> Compiler<'a> {
    /// `extend="$c.prop"`: the element's value props alias the named prop.
    pub(in crate::build) fn plan_extend_prop(
        &mut self,
        t: TemplateId,
        e: ElemId,
        p: RefId,
    ) -> Result<()> {
        let kind = self.c.templates[t].elems[e].kind;
        let display = self.c.refs[p].display.clone();
        let props = match kind {
            ComponentKind::Point => {
                let hide =
                    ComponentKind::Point.prop_defs()[crate::components::prop::point::HIDE].default;
                vec![
                    SourcePlan::coord(p, 0),
                    SourcePlan::coord(p, 1),
                    SourcePlan::Default(hide),
                ]
            }
            ComponentKind::Number | ComponentKind::NumberInput | ComponentKind::MathInput => {
                vec![SourcePlan::reference(p)]
            }
            ComponentKind::Math => {
                let id = self.c.arena.push(Expr::Cell(p as CellIdx));
                self.c.sym_text.insert(id, format!("#{p}"));
                vec![
                    SourcePlan::MathHandle(id, Post::None),
                    SourcePlan::MathValue(id),
                ]
            }
            ComponentKind::PointList => {
                self.c.templates[t].elems[e].body = Body::PointList { from: p };
                Vec::new()
            }
            _ => return Err(Error::PathTooDeep(display)),
        };
        self.c.templates[t].elems[e].props = props;
        Ok(())
    }

    /// `<graph extend="$g" name="g2"/>`: the copy's props alias the
    /// original's and its children are clones, named under the copy.
    pub(in crate::build) fn plan_container_copy(
        &mut self,
        t: TemplateId,
        e: ElemId,
        p: RefId,
    ) -> Result<()> {
        let r = self
            .plan_elem_target(t, p)
            .ok_or_else(|| Error::UncopyableKind("graph from another scope".into()))?;
        self.plan_attrs(t, e, Some(p))?;
        let scope = self.child_scope(t, e);
        let kids = self.clone_children(t, r, scope, e)?;
        self.c.templates[t].elems[e].children = kids;
        Ok(())
    }

    pub(in crate::build) fn clone_children(
        &mut self,
        t: TemplateId,
        from: ElemId,
        scope: ElemId,
        root: ElemId,
    ) -> Result<Vec<Child>> {
        let kids = self.c.templates[t].elems[from].children.clone();
        let mut out = Vec::with_capacity(kids.len());
        for ch in kids {
            match ch {
                Child::Elem(c) => {
                    let (kind, name) = {
                        let el = &self.c.templates[t].elems[c];
                        (el.kind, el.name)
                    };
                    if matches!(
                        self.c.templates[t].elems[c].body,
                        Body::Repeat { .. } | Body::Collect { .. }
                    ) {
                        return Err(Error::UncopyableKind(format!(
                            "{} inside a copied container",
                            kind.tag()
                        )));
                    }
                    let ne = self.push_elem_visible_to(t, NONE, kind, name, scope, root)?;
                    let label = format!("(copy of {})", self.elem_label(t, c));
                    self.c.refs.push(RefPlan {
                        hops: 0,
                        steps: vec![Step::Elem(c)],
                        prop: None,
                        display: label,
                    });
                    let ep = self.c.refs.len() - 1;
                    {
                        let el = &mut self.c.templates[t].elems[ne];
                        el.extend = Some(ep);
                        el.cloned = true;
                    }
                    let child_scope = if name != NONE { ne } else { scope };
                    let sub = self.clone_children(t, c, child_scope, root)?;
                    self.c.templates[t].elems[ne].children = sub;
                    self.pending_elems.push((t, ne));
                    out.push(Child::Elem(ne));
                }
                other => out.push(other),
            }
        }
        Ok(out)
    }
}
